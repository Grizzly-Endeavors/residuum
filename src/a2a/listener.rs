//! The hub's dedicated A2A protocol listener: one port, every agent under
//! `/agents/{name}/`, authenticated per request. Each agent's routes come
//! from [`crate::hub::AgentDirectory::agent_a2a_router`]. See
//! `docs/systems-usage/a2a.md`.

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
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures_util::stream::BoxStream;
use tower::ServiceExt as _;

use crate::config::A2aVisibility;
use crate::hub::{self, AgentDirectory, LifecycleError};

use super::auth::{Admission, AuthState, TunnelNonceSource, authorize};
use super::card::SharedCardState;
use super::keys_runtime::SharedA2aKeys;
use super::public_url::AGENTS_PATH_PREFIX;
use crate::remote_access::siblings::{NoSiblings, SiblingKeyVerifier};

/// A [`a2a_server::RequestHandler`] that refuses every operation with
/// `A2AError::unsupported_operation`. Production wiring uses
/// [`super::ResiduumA2aHandler`] instead; this is a lightweight stand-in for
/// this module's own tests, which only need to exercise the auth layer and
/// routing, not real task execution.
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

/// One agent's A2A routes, rooted at `/`: the Agent Card, the JSON-RPC
/// endpoint, and the REST endpoints under `/rest`, all dispatching to
/// `handler` and serving `card_state`.
///
/// This carries no authentication. The hub listener authenticates each
/// request against the agent's visibility before dispatching to it.
pub fn agent_handler_router<H: a2a_server::RequestHandler>(
    handler: Arc<H>,
    card_state: SharedCardState,
) -> Router {
    Router::new()
        .merge(a2a_server::agent_card::agent_card_router(card_state))
        .merge(a2a_server::jsonrpc::jsonrpc_router(Arc::clone(&handler)))
        .nest("/rest", a2a_server::rest::rest_router(handler))
}

/// The hub's A2A app: routes `/agents/{name}/...` to the named agent's
/// router (from [`AgentDirectory::agent_a2a_router`]) and answers everything
/// else `404`.
///
/// Per request, in order: an unknown agent is `404`; the caller is
/// authenticated against that agent's visibility (see
/// [`super::auth::authorize`]); an agent that isn't running is `503` with a
/// body saying so. The agent's router sees the request with the
/// `/agents/{name}` prefix stripped, so it is rooted at `/`.
pub fn hub_a2a_app(directory: Arc<dyn AgentDirectory>, auth: AuthState) -> Router {
    Router::new()
        .fallback(dispatch_to_agent)
        .with_state(HubA2aState { directory, auth })
}

#[derive(Clone)]
struct HubA2aState {
    directory: Arc<dyn AgentDirectory>,
    auth: AuthState,
}

/// Split `/agents/{name}[/rest]` into the agent name and the path the
/// agent's router sees (`/` when nothing follows the name).
fn split_agent_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix(AGENTS_PATH_PREFIX)?.strip_prefix('/')?;
    let (name, rest) = match after.find('/') {
        Some(idx) => after.split_at(idx),
        None => (after, "/"),
    };
    if name.is_empty() {
        return None;
    }
    Some((name, rest))
}

/// The request with its path replaced by `path` and its query preserved.
fn with_path(mut req: Request<Body>, path: &str) -> Result<Request<Body>, axum::http::Error> {
    let path_and_query = match req.uri().query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse()?);
    *req.uri_mut() = Uri::from_parts(parts)?;
    Ok(req)
}

fn visibility_for_auth(visibility: hub::A2aVisibility) -> A2aVisibility {
    match visibility {
        hub::A2aVisibility::Public => A2aVisibility::Public,
        hub::A2aVisibility::Private => A2aVisibility::Private,
    }
}

async fn dispatch_to_agent(State(state): State<HubA2aState>, req: Request<Body>) -> Response {
    let Some((name, rest)) = split_agent_path(req.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (name, rest) = (name.to_string(), rest.to_string());

    let Ok(summary) = state.directory.summary(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let req = match with_path(req, &rest) {
        Ok(req) => req,
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, "rejected an a2a request with an unusable path");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let req = match authorize(
        &state.auth,
        visibility_for_auth(summary.a2a_visibility),
        req,
    )
    .await
    {
        Admission::Admitted(req) => req,
        Admission::Answered(response) => return response,
    };

    let router = match state.directory.agent_a2a_router(&name) {
        Ok(router) => router,
        Err(LifecycleError::NotFound(_)) => return StatusCode::NOT_FOUND.into_response(),
        Err(LifecycleError::NotRunning {
            state: agent_state, ..
        }) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "agent '{name}' isn't running (it is {agent_state}); start it and try again"
                ),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(agent = %name, error = %e, "failed to reach an agent's a2a router");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("agent '{name}' couldn't handle the request"),
            )
                .into_response();
        }
    };

    match router.oneshot(req).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

/// The hub's A2A listener: one port, every agent under `/agents/{name}/`,
/// authenticated per request. See [`hub_a2a_app`].
pub struct A2aListener {
    bind: String,
    port: u16,
    directory: Arc<dyn AgentDirectory>,
    auth: AuthState,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl A2aListener {
    /// Create the listener. `bind` is the gateway's bind address; `keys` and
    /// `tunnel_nonce` are the hub-level caller-key store and tunnel nonce.
    #[must_use]
    pub fn new(
        bind: String,
        port: u16,
        directory: Arc<dyn AgentDirectory>,
        keys: SharedA2aKeys,
        tunnel_nonce: Arc<dyn TunnelNonceSource>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            bind,
            port,
            directory,
            auth: AuthState {
                keys,
                tunnel_nonce,
                sibling_keys: Arc::new(NoSiblings),
            },
            shutdown_rx,
        }
    }

    /// Accept the keys issued to joined siblings as calls from those siblings.
    #[must_use]
    pub fn with_sibling_keys(mut self, sibling_keys: Arc<dyn SiblingKeyVerifier>) -> Self {
        self.auth.sibling_keys = sibling_keys;
        self
    }

    /// Run until the shutdown signal fires.
    ///
    /// # Errors
    /// Returns an error if the listener port cannot be bound.
    pub async fn start(self) -> anyhow::Result<()> {
        let app = hub_a2a_app(self.directory, self.auth);

        let addr = format!("{}:{}", self.bind, self.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .with_context(|| format!("failed to bind the A2A listener on {addr}"))?;
        tracing::info!(addr = %addr, "a2a interface listening");

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
