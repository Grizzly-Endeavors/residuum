//! Tests for the hub A2A listener: two agents behind one port, routed by
//! `/agents/{name}/`, authenticated per request against each agent's
//! visibility, over real HTTP with the official client. See
//! `docs/systems-usage/a2a.md`.

use crate::util::test_ports::reserve_port;
use std::sync::Arc;
use std::time::Duration;

use a2a::{
    A2AError, AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest,
    GetExtendedAgentCardRequest, GetTaskPushNotificationConfigRequest, GetTaskRequest,
    ListTaskPushNotificationConfigsRequest, ListTaskPushNotificationConfigsResponse,
    ListTasksRequest, ListTasksResponse, Message, Part, Role, SendMessageRequest,
    SendMessageResponse, StreamResponse, SubscribeToTaskRequest, Task, TaskPushNotificationConfig,
};
use a2a_client::{A2AClientFactory, agent_card::AgentCardResolver, auth::AuthInterceptor};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use futures_util::stream::BoxStream;
use reqwest::StatusCode;
use tokio::sync::Notify;

use crate::a2a::auth::NoTunnel;
use crate::a2a::card::{CardRuntime, CardState};
use crate::a2a::keys_runtime::{A2aKeys, SharedA2aKeys};
use crate::a2a::listener::{A2aListener, StubHandler, agent_handler_router};
use crate::a2a::static_directory::StaticAgentDirectory;
use crate::config::A2aConfig;
use crate::hub::{A2aVisibility, AgentState};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A handler that identifies its agent in every reply and gates its second
/// streamed event on `gate`, so a test can prove the first event arrived
/// while the response was still open.
struct AgentHandler {
    name: String,
    gate: Arc<Notify>,
}

impl AgentHandler {
    fn reply(&self, text: &str) -> Message {
        Message::new(
            Role::Agent,
            vec![Part::text(format!("{} says {text}", self.name))],
        )
    }
}

#[async_trait]
impl a2a_server::RequestHandler for AgentHandler {
    async fn send_message(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        Ok(SendMessageResponse::Message(self.reply("hi")))
    }

    async fn send_streaming_message(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let first = self.reply("first");
        let second = self.reply("second");
        let gate = Arc::clone(&self.gate);
        Ok(
            futures_util::stream::once(async move { Ok(StreamResponse::Message(first)) })
                .chain(futures_util::stream::once(async move {
                    gate.notified().await;
                    Ok(StreamResponse::Message(second))
                }))
                .boxed(),
        )
    }

    async fn get_task(
        &self,
        params: &a2a_server::ServiceParams,
        req: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        a2a_server::RequestHandler::get_task(&StubHandler, params, req).await
    }

    async fn list_tasks(
        &self,
        params: &a2a_server::ServiceParams,
        req: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        a2a_server::RequestHandler::list_tasks(&StubHandler, params, req).await
    }

    async fn cancel_task(
        &self,
        params: &a2a_server::ServiceParams,
        req: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        a2a_server::RequestHandler::cancel_task(&StubHandler, params, req).await
    }

    async fn subscribe_to_task(
        &self,
        params: &a2a_server::ServiceParams,
        req: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        a2a_server::RequestHandler::subscribe_to_task(&StubHandler, params, req).await
    }

    async fn create_push_config(
        &self,
        params: &a2a_server::ServiceParams,
        req: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        a2a_server::RequestHandler::create_push_config(&StubHandler, params, req).await
    }

    async fn get_push_config(
        &self,
        params: &a2a_server::ServiceParams,
        req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        a2a_server::RequestHandler::get_push_config(&StubHandler, params, req).await
    }

    async fn list_push_configs(
        &self,
        params: &a2a_server::ServiceParams,
        req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        a2a_server::RequestHandler::list_push_configs(&StubHandler, params, req).await
    }

    async fn delete_push_config(
        &self,
        params: &a2a_server::ServiceParams,
        req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        a2a_server::RequestHandler::delete_push_config(&StubHandler, params, req).await
    }

    async fn get_extended_agent_card(
        &self,
        params: &a2a_server::ServiceParams,
        req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        a2a_server::RequestHandler::get_extended_agent_card(&StubHandler, params, req).await
    }
}

/// A live hub A2A listener over a fake directory of agents.
struct Fixture {
    port: u16,
    keys: SharedA2aKeys,
    /// The keys of joined siblings the listener also accepts.
    siblings: Arc<crate::remote_access::siblings::SiblingKeys>,
    directory: Arc<StaticAgentDirectory>,
    /// Released to let every agent's gated stream finish.
    gate: Arc<Notify>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown_tx.send(true).ok();
    }
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn key(&self) -> String {
        self.keys.create("tester", None).await.unwrap()
    }
}

/// One agent's router: a card titled `display`, URLs under `/agents/<name>`,
/// and an [`AgentHandler`].
fn agent_router(
    dir: &std::path::Path,
    name: &str,
    display: &str,
    port: u16,
    gate: &Arc<Notify>,
) -> axum::Router {
    let card_path = dir.join(format!("{name}-card.json"));
    std::fs::write(
        &card_path,
        format!(r#"{{"name": "{display}", "description": "the {name} agent", "skills": []}}"#),
    )
    .unwrap();
    let runtime = CardRuntime::from_config(
        &A2aConfig {
            enabled: true,
            port,
            public_url: None,
            visibility: crate::config::A2aVisibility::Public,
        },
        "127.0.0.1",
        name,
        None,
    );
    let card_state = CardState::load(&card_path, &runtime).unwrap();
    let handler = Arc::new(AgentHandler {
        name: name.to_string(),
        gate: Arc::clone(gate),
    });
    agent_handler_router(handler, card_state)
}

/// Two agents behind one listener: `scout` (public) and `vault` (private).
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let reservation = reserve_port();
    let port = reservation.port();
    let gate = Arc::new(Notify::new());
    let directory = Arc::new(
        StaticAgentDirectory::new()
            .with_agent(
                "scout",
                A2aVisibility::Public,
                agent_router(dir.path(), "scout", "Scout", port, &gate),
            )
            .with_agent(
                "vault",
                A2aVisibility::Private,
                agent_router(dir.path(), "vault", "Vault", port, &gate),
            ),
    );
    let keys = A2aKeys::new_shared(dir.path());
    let siblings = Arc::new(crate::remote_access::siblings::SiblingKeys::open(
        &dir.path().join("remote-access"),
    ));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = A2aListener::new(
        "127.0.0.1".to_string(),
        port,
        Arc::<StaticAgentDirectory>::clone(&directory),
        Arc::clone(&keys),
        Arc::new(NoTunnel),
        shutdown_rx,
    )
    .with_sibling_keys(
        Arc::clone(&siblings) as Arc<dyn crate::remote_access::siblings::SiblingKeyVerifier>
    );
    // `listener.start()` binds `port` for real; drop the reservation right
    // before spawning it so no other test process can take it first.
    drop(reservation);
    crate::util::spawn_in_span(listener.start());
    tokio::time::timeout(Duration::from_secs(20), async {
        while tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for the listener on port {port}"));
    Fixture {
        port,
        keys,
        siblings,
        directory,
        gate,
        shutdown_tx,
        _dir: dir,
    }
}

fn user_message(text: &str) -> SendMessageRequest {
    SendMessageRequest {
        message: Message::new(Role::User, vec![Part::text(text)]),
        configuration: None,
        metadata: None,
        tenant: None,
    }
}

async fn client_for(
    fixture: &Fixture,
    agent: &str,
    token: &str,
) -> a2a_client::A2AClient<Box<dyn a2a_client::Transport>> {
    let mut auth = reqwest::header::HeaderMap::new();
    auth.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let http = reqwest::Client::builder()
        .default_headers(auth)
        .build()
        .unwrap();
    let card = tokio::time::timeout(
        TEST_TIMEOUT,
        AgentCardResolver::new(Some(http)).resolve(&fixture.url(&format!("/agents/{agent}"))),
    )
    .await
    .expect("timed out resolving the agent card")
    .unwrap();
    A2AClientFactory::builder()
        .with_interceptor(Arc::new(AuthInterceptor::bearer(token)))
        .build()
        .create_from_card(&card)
        .await
        .unwrap()
}

fn text_of(response: &SendMessageResponse) -> &str {
    match response {
        SendMessageResponse::Message(message) => message.text().unwrap_or_default(),
        SendMessageResponse::Task(_) => "<task>",
    }
}

fn text_of_event(event: &StreamResponse) -> &str {
    match event {
        StreamResponse::Message(message) => message.text().unwrap_or_default(),
        StreamResponse::Task(_)
        | StreamResponse::StatusUpdate(_)
        | StreamResponse::ArtifactUpdate(_) => "<not a message>",
    }
}

#[tokio::test]
async fn each_agent_serves_its_own_card_with_its_own_url() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    for (agent, display) in [("scout", "Scout"), ("vault", "Vault")] {
        let resp = http
            .get(fx.url(&format!("/agents/{agent}/.well-known/agent-card.json")))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{agent}");
        let card: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            card.get("name").and_then(serde_json::Value::as_str),
            Some(display),
            "{agent} serves its own card"
        );
        let urls: Vec<&str> = card
            .get("supportedInterfaces")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|i| i.get("url").and_then(serde_json::Value::as_str))
            .collect();
        let base = fx.url(&format!("/agents/{agent}"));
        assert!(
            urls.contains(&base.as_str()) && urls.contains(&format!("{base}/rest").as_str()),
            "{agent} card must advertise its own JSON-RPC and REST URLs, got {urls:?}"
        );
    }
}

#[tokio::test]
async fn a_public_agent_serves_its_card_without_a_key_but_nothing_else() {
    let fx = fixture().await;
    let http = reqwest::Client::new();

    let card = http
        .get(fx.url("/agents/scout/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(card.status(), StatusCode::OK);

    let rpc = http
        .post(fx.url("/agents/scout"))
        .json(
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": {}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(rpc.status(), StatusCode::UNAUTHORIZED);

    let rest = http
        .get(fx.url("/agents/scout/rest/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(rest.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_private_agent_is_404_on_every_route_without_a_key_and_served_with_one() {
    let fx = fixture().await;
    let http = reqwest::Client::new();

    for path in [
        "/agents/vault/.well-known/agent-card.json",
        "/agents/vault",
        "/agents/vault/rest/tasks",
        "/agents/vault/_a2a/auth-check",
    ] {
        let resp = http.get(fx.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path} without a key");
    }
    let bad = http
        .get(fx.url("/agents/vault/.well-known/agent-card.json"))
        .bearer_auth("rsdm_a2a_notakey")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::NOT_FOUND, "an invalid key");

    let key = fx.key().await;
    let card = http
        .get(fx.url("/agents/vault/.well-known/agent-card.json"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(card.status(), StatusCode::OK);
    let check = http
        .get(fx.url("/agents/vault/_a2a/auth-check"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(check.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_joined_siblings_key_reaches_a_private_agent_and_a_missing_key_does_not() {
    use crate::remote_access::siblings::keys::{NewSibling, issue_key};
    let fx = fixture().await;
    let inbound = issue_key();
    fx.siblings
        .upsert(NewSibling {
            slug: "desktop".to_string(),
            display_name: "Desk".to_string(),
            account_uri: "https://acme.test/acct/desktop".to_string(),
            outbound_key: issue_key(),
            inbound_key: inbound.clone(),
        })
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let card_url = fx.url("/agents/vault/.well-known/agent-card.json");

    assert_eq!(
        http.get(&card_url).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "no key"
    );
    assert_eq!(
        http.get(&card_url)
            .bearer_auth(issue_key())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND,
        "a key nobody holds"
    );
    assert_eq!(
        http.get(&card_url)
            .bearer_auth(&inbound)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK,
        "the sibling's key"
    );
    // The same key calls the agent: a real message round trip as `sibling:desktop`.
    let client = client_for(&fx, "vault", &inbound).await;
    let reply = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_message(&user_message("hello from the desktop")),
    )
    .await
    .expect("timed out sending")
    .unwrap();
    assert!(
        text_of(&reply).contains("vault"),
        "the private agent answered: {}",
        text_of(&reply)
    );

    // Revoking the join closes the door again.
    fx.siblings.remove("desktop").await.unwrap();
    assert_eq!(
        http.get(&card_url)
            .bearer_auth(&inbound)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_unknown_agent_is_404_even_with_a_valid_key() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    for path in [
        "/agents/nobody/.well-known/agent-card.json",
        "/agents/nobody",
        "/agents/nobody/rest/tasks",
    ] {
        let resp = http
            .get(fx.url(path))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn a_stopped_agent_is_503_with_a_body_saying_it_isnt_running() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();
    fx.directory.set_state("scout", AgentState::Stopped);
    fx.directory.set_state("vault", AgentState::Failed);

    let resp = http
        .get(fx.url("/agents/scout/.well-known/agent-card.json"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("scout") && body.contains("isn't running") && body.contains("stopped"),
        "{body}"
    );

    let failed = http
        .get(fx.url("/agents/vault/.well-known/agent-card.json"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(failed.text().await.unwrap().contains("failed"));

    // A private agent that isn't running still doesn't reveal itself.
    let hidden = http
        .get(fx.url("/agents/vault/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    fx.directory.set_state("scout", AgentState::Running);
    let back = http
        .get(fx.url("/agents/scout/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(back.status(), StatusCode::OK, "serving again once running");
}

#[tokio::test]
async fn paths_outside_an_agent_prefix_are_404() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    for path in [
        "/",
        "/.well-known/agent-card.json",
        "/rest/tasks",
        "/_a2a/auth-check",
        "/agents",
        "/agents/",
        "/agentsscout/.well-known/agent-card.json",
        "/other/scout/.well-known/agent-card.json",
    ] {
        let resp = http
            .get(fx.url(path))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let rpc = http
        .post(fx.url("/"))
        .bearer_auth(&key)
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage"}))
        .send()
        .await
        .unwrap();
    assert_eq!(rpc.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn jsonrpc_reaches_the_named_agent_through_the_prefix() {
    let fx = fixture().await;
    let key = fx.key().await;

    let scout = client_for(&fx, "scout", &key).await;
    let vault = client_for(&fx, "vault", &key).await;
    let from_scout = tokio::time::timeout(TEST_TIMEOUT, scout.send_message(&user_message("x")))
        .await
        .unwrap()
        .unwrap();
    let from_vault = tokio::time::timeout(TEST_TIMEOUT, vault.send_message(&user_message("x")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(text_of(&from_scout), "scout says hi");
    assert_eq!(text_of(&from_vault), "vault says hi");
}

#[tokio::test]
async fn jsonrpc_streams_events_through_the_prefix_as_they_are_produced() {
    let fx = fixture().await;
    let key = fx.key().await;
    let client = client_for(&fx, "scout", &key).await;

    let mut stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&user_message("stream")),
    )
    .await
    .unwrap()
    .unwrap();

    let first = tokio::time::timeout(TEST_TIMEOUT, stream.next())
        .await
        .expect("the first event must arrive while the stream is still open")
        .unwrap()
        .unwrap();
    assert_eq!(text_of_event(&first), "scout says first");

    fx.gate.notify_one();
    let second = tokio::time::timeout(TEST_TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(text_of_event(&second), "scout says second");
    assert!(stream.next().await.is_none());
}

fn rest_body(text: &str) -> serde_json::Value {
    serde_json::json!({
        "message": {
            "messageId": "m-1",
            "role": "ROLE_USER",
            "parts": [{"text": text}]
        }
    })
}

#[tokio::test]
async fn rest_reaches_the_named_agent_through_the_prefix() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(fx.url("/agents/vault/rest/message:send"))
        .bearer_auth(&key)
        .json(&rest_body("hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("vault says hi"), "{body}");
}

#[tokio::test]
async fn rest_streams_chunks_through_the_prefix_as_they_are_produced() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    let mut resp = http
        .post(fx.url("/agents/scout/rest/message:stream"))
        .bearer_auth(&key)
        .json(&rest_body("stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let first = tokio::time::timeout(TEST_TIMEOUT, resp.chunk())
        .await
        .expect("the first chunk must arrive while the response is still open")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("scout says first"));

    fx.gate.notify_one();
    let mut tail = String::new();
    while let Some(chunk) = tokio::time::timeout(TEST_TIMEOUT, resp.chunk())
        .await
        .unwrap()
        .unwrap()
    {
        tail.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(tail.contains("scout says second"), "{tail}");
}

#[tokio::test]
async fn the_query_string_survives_prefix_stripping() {
    let fx = fixture().await;
    let key = fx.key().await;
    let http = reqwest::Client::new();

    // The stub handler answers list_tasks with an error whatever the query;
    // reaching it (not a 404 from a mangled path) is what proves dispatch.
    let resp = http
        .get(fx.url("/agents/scout/rest/tasks?pageSize=1"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
}
