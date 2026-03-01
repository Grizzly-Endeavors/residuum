//! A2A protocol server — JSON-RPC 2.0 endpoint and Agent Card serving.
//!
//! Handles `tasks/send`, `tasks/get`, and `tasks/cancel` methods.
//! Agent Card is served at `GET /.well-known/agent.json`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::channels::types::{InboundMessage, MessageOrigin, ReplyHandle, RoutedMessage};

use super::types::{
    A2aMessage, A2aRole, AgentCard, Artifact, JsonRpcErrorResponse, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, Part, Task, TaskCancelParams, TaskGetParams, TaskSendParams, TaskState,
    TaskStatus, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, TASK_NOT_FOUND,
};

// ── Task Store ──────────────────────────────────────────────────────────

/// In-memory task store for tracking A2A task state.
///
/// Tasks are transient and do not survive restarts.
type SharedTaskStore = Arc<Mutex<HashMap<String, Task>>>;

// ── Reply Handle ────────────────────────────────────────────────────────

/// Reply handle for A2A tasks.
///
/// Collects agent response text and signals completion via a oneshot channel.
struct A2aReplyHandle {
    /// Accumulated response fragments.
    response_parts: Mutex<Vec<String>>,
    /// Signals when the agent turn is complete.
    done_tx: Mutex<Option<oneshot::Sender<Vec<String>>>>,
}

impl A2aReplyHandle {
    fn new(done_tx: oneshot::Sender<Vec<String>>) -> Self {
        Self {
            response_parts: Mutex::new(Vec::new()),
            done_tx: Mutex::new(Some(done_tx)),
        }
    }
}

#[async_trait::async_trait]
impl ReplyHandle for A2aReplyHandle {
    async fn send_response(&self, content: &str) {
        let mut parts = self.response_parts.lock().await;
        parts.push(content.to_string());

        // Signal completion after the response is collected
        let collected = parts.clone();
        drop(parts);

        let mut done_guard = self.done_tx.lock().await;
        if let Some(tx) = done_guard.take() {
            tx.send(collected).ok();
        }
    }

    async fn send_typing(&self) {
        // No typing indicator for A2A
    }

    async fn send_system_event(&self, source: &str, content: &str) {
        tracing::debug!(
            channel = "a2a",
            source,
            event_len = content.len(),
            "a2a system event"
        );
    }
}

// ── Shared State ────────────────────────────────────────────────────────

/// Shared state for the A2A axum routes.
#[derive(Clone)]
pub(crate) struct A2aState {
    /// Agent card to serve at the well-known endpoint.
    pub agent_card: Arc<AgentCard>,
    /// Sender for routing messages to the gateway main loop.
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    /// In-memory task store.
    pub task_store: SharedTaskStore,
    /// Optional bearer token for authentication.
    pub secret: Option<String>,
}

// ── Router ──────────────────────────────────────────────────────────────

/// Build the A2A protocol router.
///
/// Mounts:
/// - `GET /.well-known/agent.json` — Agent Card discovery
/// - `POST /a2a` — JSON-RPC 2.0 endpoint
#[must_use]
pub(crate) fn a2a_router(state: A2aState) -> axum::Router {
    axum::Router::new()
        .route("/.well-known/agent.json", get(agent_card_handler))
        .route("/a2a", post(jsonrpc_handler))
        .with_state(state)
}

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /.well-known/agent.json` — serve the Agent Card.
async fn agent_card_handler(State(state): State<A2aState>) -> Json<AgentCard> {
    Json((*state.agent_card).clone())
}

/// `POST /a2a` — JSON-RPC 2.0 dispatch.
async fn jsonrpc_handler(
    State(state): State<A2aState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    // Auth check
    if let Some(ref expected) = state.secret {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let provided = auth.strip_prefix("Bearer ").unwrap_or("");
        if provided != expected.as_str() {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }

    // Parse JSON-RPC request
    let request: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(req) => req,
        Err(e) => {
            let resp = JsonRpcErrorResponse::new(
                JsonRpcId::Number(0),
                INVALID_REQUEST,
                format!("invalid JSON-RPC request: {e}"),
            );
            return Json(resp).into_response();
        }
    };

    if request.jsonrpc != "2.0" {
        let resp = JsonRpcErrorResponse::new(
            request.id,
            INVALID_REQUEST,
            "jsonrpc version must be \"2.0\"",
        );
        return Json(resp).into_response();
    }

    // Dispatch to method handler
    let result = match request.method.as_str() {
        "tasks/send" => handle_tasks_send(&state, request.id.clone(), request.params).await,
        "tasks/get" => handle_tasks_get(&state, request.id.clone(), request.params).await,
        "tasks/cancel" => handle_tasks_cancel(&state, request.id.clone(), request.params).await,
        _ => Err(JsonRpcErrorResponse::new(
            request.id,
            METHOD_NOT_FOUND,
            format!("method '{}' not found", request.method),
        )),
    };

    match result {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => Json(err).into_response(),
    }
}

// ── Method Handlers ─────────────────────────────────────────────────────

/// Handle `tasks/send`: submit a message and wait for the agent's response.
async fn handle_tasks_send(
    state: &A2aState,
    rpc_id: JsonRpcId,
    params: serde_json::Value,
) -> Result<JsonRpcResponse, JsonRpcErrorResponse> {
    let send_params: TaskSendParams = serde_json::from_value(params).map_err(|e| {
        JsonRpcErrorResponse::new(
            rpc_id.clone(),
            INVALID_PARAMS,
            format!("invalid params for tasks/send: {e}"),
        )
    })?;

    // Extract text content from message parts
    let content = extract_text_content(&send_params.message);
    if content.is_empty() {
        return Err(JsonRpcErrorResponse::new(
            rpc_id,
            INVALID_PARAMS,
            "message must contain at least one text part",
        ));
    }

    let task_id = send_params.id.clone();

    // Create or update task in store
    {
        let mut store = state.task_store.lock().await;
        let task = store.entry(task_id.clone()).or_insert_with(|| Task {
            id: task_id.clone(),
            session_id: send_params.session_id.clone(),
            status: TaskStatus {
                state: TaskState::Submitted,
                message: None,
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            history: Vec::new(),
            artifacts: Vec::new(),
            metadata: send_params.metadata.clone(),
        });

        // Add user message to history
        task.history.push(send_params.message.clone());
        task.status = TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        };
    }

    // Set up completion channel
    let (done_tx, done_rx) = oneshot::channel();
    let reply_handle = Arc::new(A2aReplyHandle::new(done_tx));

    // Route message to the gateway
    let origin = MessageOrigin {
        channel: "a2a".to_string(),
        sender_name: "a2a-client".to_string(),
        sender_id: send_params
            .session_id
            .clone()
            .unwrap_or_else(|| task_id.clone()),
    };

    let inbound = InboundMessage {
        id: task_id.clone(),
        content,
        origin,
        timestamp: chrono::Utc::now(),
    };

    let routed = RoutedMessage {
        message: inbound,
        reply: reply_handle,
    };

    if state.inbound_tx.send(routed).await.is_err() {
        // Update task to failed
        let mut store = state.task_store.lock().await;
        if let Some(task) = store.get_mut(&task_id) {
            task.status = TaskStatus {
                state: TaskState::Failed,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![Part::Text {
                        text: "gateway is unavailable".to_string(),
                    }],
                    metadata: HashMap::new(),
                }),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            };
        }
        return Err(JsonRpcErrorResponse::new(
            rpc_id,
            INTERNAL_ERROR,
            "gateway is unavailable",
        ));
    }

    // Wait for agent response (with timeout)
    let response_texts =
        match tokio::time::timeout(tokio::time::Duration::from_secs(300), done_rx).await {
            Ok(Ok(texts)) => texts,
            Ok(Err(_)) => {
                // Sender dropped without sending — agent turn errored
                update_task_failed(&state.task_store, &task_id, "agent turn failed").await;
                return Err(JsonRpcErrorResponse::new(
                    rpc_id,
                    INTERNAL_ERROR,
                    "agent turn failed",
                ));
            }
            Err(_) => {
                // Timeout
                update_task_failed(&state.task_store, &task_id, "request timed out").await;
                return Err(JsonRpcErrorResponse::new(
                    rpc_id,
                    INTERNAL_ERROR,
                    "request timed out after 300 seconds",
                ));
            }
        };

    // Build response artifact
    let response_text = response_texts.join("\n");
    let agent_message = A2aMessage {
        role: A2aRole::Agent,
        parts: vec![Part::Text {
            text: response_text.clone(),
        }],
        metadata: HashMap::new(),
    };
    let artifact = Artifact {
        name: Some("response".to_string()),
        description: None,
        parts: vec![Part::Text {
            text: response_text,
        }],
        index: Some(0),
    };

    // Update task to completed
    let task = {
        let mut store = state.task_store.lock().await;
        if let Some(task) = store.get_mut(&task_id) {
            task.history.push(agent_message.clone());
            task.artifacts.push(artifact);
            task.status = TaskStatus {
                state: TaskState::Completed,
                message: Some(agent_message),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            };
            task.clone()
        } else {
            return Err(JsonRpcErrorResponse::new(
                rpc_id,
                INTERNAL_ERROR,
                "task disappeared from store",
            ));
        }
    };

    let task_json = serde_json::to_value(&task).map_err(|e| {
        JsonRpcErrorResponse::new(rpc_id.clone(), INTERNAL_ERROR, format!("serialization error: {e}"))
    })?;

    Ok(JsonRpcResponse::success(rpc_id, task_json))
}

/// Handle `tasks/get`: retrieve current task state.
async fn handle_tasks_get(
    state: &A2aState,
    rpc_id: JsonRpcId,
    params: serde_json::Value,
) -> Result<JsonRpcResponse, JsonRpcErrorResponse> {
    let get_params: TaskGetParams = serde_json::from_value(params).map_err(|e| {
        JsonRpcErrorResponse::new(
            rpc_id.clone(),
            INVALID_PARAMS,
            format!("invalid params for tasks/get: {e}"),
        )
    })?;

    let store = state.task_store.lock().await;
    let task = store.get(&get_params.id).ok_or_else(|| {
        JsonRpcErrorResponse::new(
            rpc_id.clone(),
            TASK_NOT_FOUND,
            format!("task '{}' not found", get_params.id),
        )
    })?;

    // Apply history length limit if requested
    let mut task_clone = task.clone();
    if let Some(max_len) = get_params.history_length {
        let len = task_clone.history.len();
        if len > max_len {
            task_clone.history = task_clone.history.split_off(len - max_len);
        }
    }

    let task_json = serde_json::to_value(&task_clone).map_err(|e| {
        JsonRpcErrorResponse::new(rpc_id.clone(), INTERNAL_ERROR, format!("serialization error: {e}"))
    })?;

    Ok(JsonRpcResponse::success(rpc_id, task_json))
}

/// Handle `tasks/cancel`: cancel a running task.
async fn handle_tasks_cancel(
    state: &A2aState,
    rpc_id: JsonRpcId,
    params: serde_json::Value,
) -> Result<JsonRpcResponse, JsonRpcErrorResponse> {
    let cancel_params: TaskCancelParams = serde_json::from_value(params).map_err(|e| {
        JsonRpcErrorResponse::new(
            rpc_id.clone(),
            INVALID_PARAMS,
            format!("invalid params for tasks/cancel: {e}"),
        )
    })?;

    let mut store = state.task_store.lock().await;
    let task = store.get_mut(&cancel_params.id).ok_or_else(|| {
        JsonRpcErrorResponse::new(
            rpc_id.clone(),
            TASK_NOT_FOUND,
            format!("task '{}' not found", cancel_params.id),
        )
    })?;

    // Only working or submitted tasks can be cancelled
    match task.status.state {
        TaskState::Working | TaskState::Submitted | TaskState::InputRequired => {
            task.status = TaskStatus {
                state: TaskState::Canceled,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![Part::Text {
                        text: "task cancelled by client".to_string(),
                    }],
                    metadata: HashMap::new(),
                }),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            };
            let task_json = serde_json::to_value(&*task).map_err(|e| {
                JsonRpcErrorResponse::new(
                    rpc_id.clone(),
                    INTERNAL_ERROR,
                    format!("serialization error: {e}"),
                )
            })?;
            Ok(JsonRpcResponse::success(rpc_id, task_json))
        }
        _ => Err(JsonRpcErrorResponse::new(
            rpc_id,
            super::types::TASK_NOT_CANCELABLE,
            format!(
                "task '{}' cannot be cancelled (state: {:?})",
                cancel_params.id, task.status.state
            ),
        )),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Extract concatenated text content from an A2A message's parts.
fn extract_text_content(message: &A2aMessage) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Update a task's status to failed with a given reason.
async fn update_task_failed(store: &SharedTaskStore, task_id: &str, reason: &str) {
    let mut store = store.lock().await;
    if let Some(task) = store.get_mut(task_id) {
        task.status = TaskStatus {
            state: TaskState::Failed,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![Part::Text {
                    text: reason.to_string(),
                }],
                metadata: HashMap::new(),
            }),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        };
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn make_app(
        secret: Option<String>,
    ) -> (axum::Router, mpsc::Receiver<RoutedMessage>, SharedTaskStore) {
        let (tx, rx) = mpsc::channel::<RoutedMessage>(32);
        let task_store: SharedTaskStore = Arc::new(Mutex::new(HashMap::new()));

        let card = AgentCard {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            url: "http://localhost:7700/a2a".to_string(),
            version: "0.2".to_string(),
            capabilities: None,
            skills: vec![],
            default_input_modes: vec!["text/plain".to_string()],
            default_output_modes: vec!["text/plain".to_string()],
            authentication: None,
        };

        let state = A2aState {
            agent_card: Arc::new(card),
            inbound_tx: tx,
            task_store: Arc::clone(&task_store),
            secret,
        };

        let app = a2a_router(state);
        (app, rx, task_store)
    }

    #[tokio::test]
    async fn agent_card_served_at_well_known() {
        let (app, _rx, _store) = make_app(None);

        let req = Request::builder()
            .method("GET")
            .uri("/.well-known/agent.json")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "should return 200");

        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let card: AgentCard = serde_json::from_slice(&body).unwrap();
        assert_eq!(card.name, "Test Agent", "agent name should match");
    }

    #[tokio::test]
    async fn jsonrpc_method_not_found() {
        let (app, _rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "unknown/method",
            "params": {}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "JSON-RPC errors return 200");

        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            error_resp
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_i64()),
            Some(i64::from(METHOD_NOT_FOUND)),
            "should return method not found error"
        );
    }

    #[tokio::test]
    async fn jsonrpc_auth_required() {
        let (app, _rx, _store) = make_app(Some("secret-token".to_string()));

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/get",
            "params": {"id": "test"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "missing auth should be rejected"
        );
    }

    #[tokio::test]
    async fn jsonrpc_auth_correct_token() {
        let (app, _rx, _store) = make_app(Some("secret-token".to_string()));

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/get",
            "params": {"id": "nonexistent"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .header("authorization", "Bearer secret-token")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // Should return 200 (JSON-RPC error in body, not HTTP error)
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "correct auth should pass through"
        );
    }

    #[tokio::test]
    async fn tasks_get_not_found() {
        let (app, _rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/get",
            "params": {"id": "nonexistent"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            error_resp
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_i64()),
            Some(i64::from(TASK_NOT_FOUND)),
            "should return task not found"
        );
    }

    #[tokio::test]
    async fn tasks_get_returns_existing_task() {
        let (app, _rx, store) = make_app(None);

        // Pre-populate a task
        {
            let mut s = store.lock().await;
            s.insert(
                "task-1".to_string(),
                Task {
                    id: "task-1".to_string(),
                    session_id: None,
                    status: TaskStatus {
                        state: TaskState::Completed,
                        message: None,
                        timestamp: None,
                    },
                    history: vec![A2aMessage {
                        role: A2aRole::User,
                        parts: vec![Part::Text {
                            text: "hello".to_string(),
                        }],
                        metadata: HashMap::new(),
                    }],
                    artifacts: vec![],
                    metadata: HashMap::new(),
                },
            );
        }

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tasks/get",
            "params": {"id": "task-1"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let success_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let result = success_resp.get("result").unwrap();
        assert_eq!(
            result.get("id").and_then(|v| v.as_str()),
            Some("task-1"),
            "task id should match"
        );
        assert_eq!(
            result
                .get("status")
                .and_then(|s| s.get("state"))
                .and_then(|v| v.as_str()),
            Some("completed"),
            "task state should be completed"
        );
    }

    #[tokio::test]
    async fn tasks_cancel_nonexistent() {
        let (app, _rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/cancel",
            "params": {"id": "nonexistent"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            error_resp
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_i64()),
            Some(i64::from(TASK_NOT_FOUND)),
            "should return task not found"
        );
    }

    #[tokio::test]
    async fn tasks_cancel_working_task() {
        let (app, _rx, store) = make_app(None);

        // Pre-populate a working task
        {
            let mut s = store.lock().await;
            s.insert(
                "task-2".to_string(),
                Task {
                    id: "task-2".to_string(),
                    session_id: None,
                    status: TaskStatus {
                        state: TaskState::Working,
                        message: None,
                        timestamp: None,
                    },
                    history: vec![],
                    artifacts: vec![],
                    metadata: HashMap::new(),
                },
            );
        }

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/cancel",
            "params": {"id": "task-2"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let success_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let result = success_resp.get("result").unwrap();
        assert_eq!(
            result
                .get("status")
                .and_then(|s| s.get("state"))
                .and_then(|v| v.as_str()),
            Some("canceled"),
            "task should be cancelled"
        );
    }

    #[tokio::test]
    async fn tasks_cancel_completed_task_fails() {
        let (app, _rx, store) = make_app(None);

        // Pre-populate a completed task
        {
            let mut s = store.lock().await;
            s.insert(
                "task-3".to_string(),
                Task {
                    id: "task-3".to_string(),
                    session_id: None,
                    status: TaskStatus {
                        state: TaskState::Completed,
                        message: None,
                        timestamp: None,
                    },
                    history: vec![],
                    artifacts: vec![],
                    metadata: HashMap::new(),
                },
            );
        }

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/cancel",
            "params": {"id": "task-3"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error_resp.get("error").is_some(),
            "cancelling completed task should error"
        );
    }

    #[tokio::test]
    async fn tasks_send_routes_to_inbound() {
        let (app, mut rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/send",
            "params": {
                "id": "task-send-1",
                "message": {
                    "role": "user",
                    "parts": [{"type": "text", "text": "hello from a2a"}]
                }
            }
        });

        // Spawn the request handler (it will block waiting for agent response)
        let handle = tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();

            app.oneshot(req).await.unwrap()
        });

        // Receive the routed message and simulate agent response
        let routed = rx.recv().await.unwrap();
        assert_eq!(
            routed.message.content, "hello from a2a",
            "content should match"
        );
        assert_eq!(
            routed.message.origin.channel, "a2a",
            "channel should be a2a"
        );

        // Simulate agent response
        routed.reply.send_response("hello back from agent").await;

        // Check the JSON-RPC response
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "should return 200");

        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let success_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let result = success_resp.get("result").unwrap();
        assert_eq!(
            result
                .get("status")
                .and_then(|s| s.get("state"))
                .and_then(|v| v.as_str()),
            Some("completed"),
            "task should be completed"
        );

        // Check artifacts
        let artifacts = result.get("artifacts").and_then(|a| a.as_array()).unwrap();
        assert_eq!(artifacts.len(), 1, "should have one artifact");
        let first_part = artifacts
            .first()
            .and_then(|a| a.get("parts"))
            .and_then(|p| p.as_array())
            .and_then(|p| p.first())
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str());
        assert_eq!(
            first_part,
            Some("hello back from agent"),
            "artifact text should match agent response"
        );
    }

    #[tokio::test]
    async fn tasks_send_empty_message_rejected() {
        let (app, _rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "tasks/send",
            "params": {
                "id": "task-empty",
                "message": {
                    "role": "user",
                    "parts": [{"type": "data", "data": {}}]
                }
            }
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error_resp.get("error").is_some(),
            "empty text should be rejected"
        );
    }

    #[test]
    fn extract_text_content_from_parts() {
        let msg = A2aMessage {
            role: A2aRole::User,
            parts: vec![
                Part::Text {
                    text: "first".to_string(),
                },
                Part::Data {
                    data: serde_json::json!({}),
                },
                Part::Text {
                    text: "second".to_string(),
                },
            ],
            metadata: HashMap::new(),
        };

        let text = extract_text_content(&msg);
        assert_eq!(text, "first\nsecond", "should join text parts with newline");
    }

    #[test]
    fn extract_text_content_no_text_parts() {
        let msg = A2aMessage {
            role: A2aRole::User,
            parts: vec![Part::Data {
                data: serde_json::json!({}),
            }],
            metadata: HashMap::new(),
        };

        let text = extract_text_content(&msg);
        assert!(text.is_empty(), "should be empty with no text parts");
    }

    #[tokio::test]
    async fn invalid_json_returns_error() {
        let (app, _rx, _store) = make_app(None);

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from("not json at all"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error_resp.get("error").is_some(),
            "invalid JSON should return error"
        );
    }

    #[tokio::test]
    async fn wrong_jsonrpc_version() {
        let (app, _rx, _store) = make_app(None);

        let body = serde_json::json!({
            "jsonrpc": "1.0",
            "id": "req-1",
            "method": "tasks/get",
            "params": {"id": "test"}
        });

        let req = Request::builder()
            .method("POST")
            .uri("/a2a")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let error_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error_resp.get("error").is_some(),
            "wrong version should return error"
        );
    }
}
