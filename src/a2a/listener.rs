//! The dedicated A2A protocol listener: card router, JSON-RPC and REST
//! routers over a pluggable [`a2a_server::RequestHandler`], and the auth
//! layer — following `TeamsInterface::start`
//! (`src/interfaces/teams/mod.rs`). See `docs/systems-usage/a2a.md`.

use std::sync::Arc;

use a2a::{
    A2AError, AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest,
    GetExtendedAgentCardRequest, GetTaskPushNotificationConfigRequest, GetTaskRequest,
    ListTaskPushNotificationConfigsRequest, ListTaskPushNotificationConfigsResponse,
    ListTasksRequest, ListTasksResponse, SendMessageRequest, SendMessageResponse, StreamResponse,
    SubscribeToTaskRequest, Task, TaskPushNotificationConfig,
};
use anyhow::Context as _;
use async_trait::async_trait;
use futures_util::stream::BoxStream;

use crate::config::A2aConfig;

use super::auth::{AuthState, TunnelNonceSource, auth_middleware};
use super::card::SharedCardState;
use super::keys_runtime::SharedA2aKeys;

/// Placeholder [`a2a_server::RequestHandler`] that refuses every operation
/// with `A2AError::unsupported_operation`. Swapped for a handler wrapping
/// the session executor and persistent task store once those exist —
/// nothing else in this module needs to change to make that swap.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubHandler;

fn unsupported(operation: &str) -> A2AError {
    A2AError::unsupported_operation(format!(
        "A2A support has no session executor wired up yet ({operation})"
    ))
}

#[async_trait]
impl a2a_server::RequestHandler for StubHandler {
    async fn send_message(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        Err(unsupported("send_message"))
    }

    async fn send_streaming_message(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        Err(unsupported("send_streaming_message"))
    }

    async fn get_task(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        Err(unsupported("get_task"))
    }

    async fn list_tasks(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        Err(unsupported("list_tasks"))
    }

    async fn cancel_task(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        Err(unsupported("cancel_task"))
    }

    async fn subscribe_to_task(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        Err(unsupported("subscribe_to_task"))
    }

    async fn create_push_config(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        Err(unsupported("create_push_config"))
    }

    async fn get_push_config(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        Err(unsupported("get_push_config"))
    }

    async fn list_push_configs(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        Err(unsupported("list_push_configs"))
    }

    async fn delete_push_config(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        Err(unsupported("delete_push_config"))
    }

    async fn get_extended_agent_card(
        &self,
        _params: &a2a_server::ServiceParams,
        _req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        Err(unsupported("get_extended_agent_card"))
    }
}

/// The A2A adapter: card, JSON-RPC, and REST routers behind the auth layer,
/// dispatching to a pluggable [`a2a_server::RequestHandler`].
pub struct A2aListener<H: a2a_server::RequestHandler> {
    cfg: A2aConfig,
    bind: String,
    handler: Arc<H>,
    card_state: SharedCardState,
    keys: SharedA2aKeys,
    tunnel_nonce: Arc<dyn TunnelNonceSource>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl<H: a2a_server::RequestHandler> A2aListener<H> {
    /// Create the listener; `bind` is the gateway's bind address.
    #[must_use]
    pub fn new(
        cfg: A2aConfig,
        bind: String,
        handler: Arc<H>,
        card_state: SharedCardState,
        keys: SharedA2aKeys,
        tunnel_nonce: Arc<dyn TunnelNonceSource>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            cfg,
            bind,
            handler,
            card_state,
            keys,
            tunnel_nonce,
            shutdown_rx,
        }
    }

    /// Run until the shutdown signal fires.
    ///
    /// # Errors
    /// Returns an error if the listener port cannot be bound.
    pub async fn start(self) -> anyhow::Result<()> {
        let auth_state = AuthState {
            keys: self.keys,
            tunnel_nonce: self.tunnel_nonce,
            visibility: self.cfg.visibility,
        };

        let app = axum::Router::new()
            .merge(a2a_server::agent_card::agent_card_router(self.card_state))
            .merge(a2a_server::jsonrpc::jsonrpc_router(Arc::clone(
                &self.handler,
            )))
            .nest("/rest", a2a_server::rest::rest_router(self.handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                auth_middleware,
            ));

        let addr = format!("{}:{}", self.bind, self.cfg.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .with_context(|| format!("failed to bind the A2A listener on {addr}"))?;
        tracing::info!(
            addr = %addr,
            visibility = %self.cfg.visibility,
            "a2a interface listening"
        );

        let mut shutdown_rx = self.shutdown_rx;
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                if shutdown_rx.changed().await.is_err() {
                    tracing::debug!("a2a shutdown sender dropped; stopping listener");
                }
            })
            .await;

        tracing::info!("a2a interface stopped");
        served.context("a2a listener failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::auth::{AUTH_CHECK_PATH, NoTunnel};
    use crate::a2a::card::{CardRuntime, CardState};
    use crate::a2a::keys_runtime::A2aKeys;
    use crate::config::A2aVisibility;

    fn card_runtime(port: u16) -> CardRuntime {
        CardRuntime {
            interfaces_base_url: format!("http://127.0.0.1:{port}"),
            visibility: A2aVisibility::Public,
        }
    }

    /// A port that was free at the moment this returned. The listener under
    /// test binds it right after, matching the pattern used elsewhere in
    /// this crate (e.g. `workbench::server` tests) for a component that
    /// takes a port number rather than a pre-bound listener.
    async fn free_port() -> u16 {
        tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn spawn_listener(
        port: u16,
        visibility: A2aVisibility,
    ) -> (SharedA2aKeys, tokio::sync::watch::Sender<bool>) {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("agent-card.json");
        std::fs::write(
            &card_path,
            r#"{"name": "Test Agent", "description": "a stub-backed test agent", "skills": []}"#,
        )
        .unwrap();
        let card_state = CardState::load(&card_path, &card_runtime(port)).unwrap();
        let keys = A2aKeys::new_shared(dir.path());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let cfg = A2aConfig {
            enabled: true,
            port,
            public_url: None,
            visibility,
        };
        let listener = A2aListener::new(
            cfg,
            "127.0.0.1".to_string(),
            Arc::new(StubHandler),
            card_state,
            Arc::clone(&keys),
            Arc::new(NoTunnel),
            shutdown_rx,
        );
        tokio::spawn(listener.start());
        // Give the listener a moment to bind before the test issues requests.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (keys, shutdown_tx)
    }

    #[tokio::test]
    async fn card_is_served_and_stub_handler_refuses_jsonrpc() {
        let port = free_port().await;
        let (_keys, shutdown_tx) = spawn_listener(port, A2aVisibility::Public).await;

        let client = reqwest::Client::new();
        let card_resp = client
            .get(format!(
                "http://127.0.0.1:{port}/.well-known/agent-card.json"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(card_resp.status(), reqwest::StatusCode::OK);
        let card_json: serde_json::Value = card_resp.json().await.unwrap();
        assert_eq!(
            card_json.get("name").and_then(|v| v.as_str()),
            Some("Test Agent")
        );

        let auth_check = client
            .get(format!("http://127.0.0.1:{port}{AUTH_CHECK_PATH}"))
            .send()
            .await
            .unwrap();
        assert_eq!(auth_check.status(), reqwest::StatusCode::NOT_FOUND);

        let rpc = client
            .post(format!("http://127.0.0.1:{port}/"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "message/send",
                "params": {"message": {"role": "user", "parts": [{"kind": "text", "text": "hi"}]}}
            }))
            .send()
            .await
            .unwrap();
        // Unauthenticated in public mode: the auth layer refuses before the
        // stub handler is ever reached.
        assert_eq!(rpc.status(), reqwest::StatusCode::UNAUTHORIZED);

        shutdown_tx.send(true).ok();
    }

    #[tokio::test]
    async fn valid_key_reaches_the_stub_handler_and_gets_unsupported_operation() {
        let port = free_port().await;
        let (keys, shutdown_tx) = spawn_listener(port, A2aVisibility::Public).await;
        let token = keys.create("caller", None).await.unwrap();

        let client = reqwest::Client::new();
        let rpc = client
            .post(format!("http://127.0.0.1:{port}/"))
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "message/send",
                "params": {"message": {"role": "user", "parts": [{"kind": "text", "text": "hi"}]}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            rpc.status(),
            reqwest::StatusCode::OK,
            "jsonrpc errors are 200 with an error envelope"
        );
        let body: serde_json::Value = rpc.json().await.unwrap();
        assert!(
            body.get("error").is_some(),
            "the stub handler must refuse every operation: {body}"
        );

        shutdown_tx.send(true).ok();
    }

    #[tokio::test]
    async fn private_visibility_hides_the_card_without_a_key() {
        let port = free_port().await;
        let (_keys, shutdown_tx) = spawn_listener(port, A2aVisibility::Private).await;

        let resp = reqwest::get(format!(
            "http://127.0.0.1:{port}/.well-known/agent-card.json"
        ))
        .await
        .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

        shutdown_tx.send(true).ok();
    }
}
