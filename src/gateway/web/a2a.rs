//! A2A caller-key management API endpoints, and the client-side "remote
//! agents" endpoints: `GET /api/a2a/agents` (live status) and
//! `GET`/`PUT /api/a2a/agents/raw` (the `config/a2a.json` editor).
//!
//! Each request opens its own handle on the key store; writes are serialized
//! across handles and processes by the store's file lock, so the web UI,
//! the CLI, and the running A2A listener never lose each other's updates.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use crate::a2a::{
    A2aClientHub, A2aKeyError, A2aKeyInfo, A2aKeys, AgentSnapshot, AgentSource, AgentStatus,
};

use super::ConfigApiState;
use super::config::ValidateResponse;

/// Request body for `POST /api/a2a/keys`.
#[derive(Deserialize)]
pub(super) struct CreateA2aKeyRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response from `POST /api/a2a/keys`. Carries the token — shown only here,
/// once, and never again.
#[derive(Serialize)]
pub(super) struct CreateA2aKeyResponse {
    pub name: String,
    pub token: String,
}

/// Response from `GET /api/a2a/keys`.
#[derive(Serialize)]
pub(super) struct ListA2aKeysResponse {
    pub keys: Vec<A2aKeyInfo>,
}

/// Response from `DELETE /api/a2a/keys/{name}`.
#[derive(Serialize)]
pub(super) struct RevokeA2aKeyResponse {
    pub revoked: bool,
}

fn error_response(e: &A2aKeyError) -> (StatusCode, String) {
    let status = match e {
        A2aKeyError::Invalid(_) => StatusCode::BAD_REQUEST,
        A2aKeyError::NotFound(_) => StatusCode::NOT_FOUND,
        A2aKeyError::AlreadyExists(_) => StatusCode::CONFLICT,
        A2aKeyError::Storage(_) => {
            tracing::error!(error = %e, "A2A caller key store request failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, e.to_string())
}

/// `GET /api/a2a/keys` — list caller keys (metadata only, never tokens).
pub(super) async fn api_a2a_keys_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<ListA2aKeysResponse>, (StatusCode, String)> {
    let snapshot = A2aKeys::new(state.config_dir)
        .snapshot()
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(ListA2aKeysResponse {
        keys: snapshot.list(),
    }))
}

/// `POST /api/a2a/keys` — mint a caller key, returning the token once.
pub(super) async fn api_a2a_keys_create(
    State(state): State<ConfigApiState>,
    Json(req): Json<CreateA2aKeyRequest>,
) -> Result<Json<CreateA2aKeyResponse>, (StatusCode, String)> {
    let token = A2aKeys::new(state.config_dir)
        .create(&req.name, req.description.as_deref())
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(CreateA2aKeyResponse {
        name: req.name,
        token,
    }))
}

/// `DELETE /api/a2a/keys/{name}` — revoke a caller key.
pub(super) async fn api_a2a_keys_revoke(
    State(state): State<ConfigApiState>,
    Path(name): Path<String>,
) -> Result<Json<RevokeA2aKeyResponse>, (StatusCode, String)> {
    A2aKeys::new(state.config_dir)
        .revoke(&name)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(RevokeA2aKeyResponse { revoked: true }))
}

// ── Remote agents (client side) ─────────────────────────────────────────

/// Shared state for `GET /api/a2a/agents`.
#[derive(Clone)]
pub(crate) struct A2aAgentsStatusState {
    pub hub: Arc<A2aClientHub>,
}

/// Build the remote-agents status API router.
pub(crate) fn a2a_agents_status_router(state: A2aAgentsStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/a2a/agents", axum::routing::get(api_a2a_agents_list))
        .with_state(state)
}

/// One skill on a remote agent's card, in the shape the web UI wants.
#[derive(Serialize)]
pub(crate) struct A2aAgentSkillView {
    id: String,
    name: String,
}

/// One remote agent's card, in the shape the web UI wants.
#[derive(Serialize)]
pub(crate) struct A2aAgentCardView {
    name: String,
    description: String,
    skills: Vec<A2aAgentSkillView>,
}

/// One entry in `GET /api/a2a/agents`.
#[derive(Serialize)]
pub(crate) struct A2aAgentView {
    name: String,
    url: String,
    source: AgentSource,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    card: Option<A2aAgentCardView>,
}

impl From<AgentSnapshot> for A2aAgentView {
    fn from(snapshot: AgentSnapshot) -> Self {
        let (status, error, card) = match &snapshot.status {
            AgentStatus::Pending => ("pending", None, None),
            AgentStatus::Ok(card) => (
                "ok",
                None,
                Some(A2aAgentCardView {
                    name: card.name.clone(),
                    description: card.description.clone(),
                    skills: card
                        .skills
                        .iter()
                        .map(|s| A2aAgentSkillView {
                            id: s.id.clone(),
                            name: s.name.clone(),
                        })
                        .collect(),
                }),
            ),
            AgentStatus::Error(e) => ("error", Some(e.clone()), None),
        };
        Self {
            name: snapshot.name,
            url: snapshot.url,
            source: snapshot.source,
            status,
            error,
            card,
        }
    }
}

/// `GET /api/a2a/agents` — every registered remote agent's live status.
pub(crate) async fn api_a2a_agents_list(
    State(state): State<A2aAgentsStatusState>,
) -> Json<Vec<A2aAgentView>> {
    let agents = state.hub.snapshot().await;
    Json(agents.into_iter().map(A2aAgentView::from).collect())
}

/// Default `config/a2a.json` content when the file doesn't exist yet.
const DEFAULT_A2A_AGENTS_JSON: &str = r#"{"agents":{}}"#;

/// `GET /api/a2a/agents/raw` — return raw `config/a2a.json` contents.
pub(super) async fn api_a2a_agents_raw_get(
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let path =
        crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    let contents = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_A2A_AGENTS_JSON.to_string(),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read a2a.json: {e}"),
            ));
        }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(contents))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("response build error: {e}"),
            )
        })
}

/// `PUT /api/a2a/agents/raw` — validate JSON + schema, write `config/a2a.json`
/// atomically, and trigger a workspace reload.
pub(super) async fn api_a2a_agents_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    crate::a2a::validate_a2a_agents_json(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(e),
            }),
        )
    })?;

    let path =
        crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    crate::util::fs::atomic_write(&path, body.as_bytes())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write a2a.json: {e}")),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(dir: &std::path::Path) -> ConfigApiState {
        ConfigApiState {
            config_dir: dir.to_path_buf(),
            workspace_dir: dir.join("workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[tokio::test]
    async fn create_list_revoke_roundtrip_returns_token_only_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let created = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: Some("my other instance".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(created.name, "laptop");
        assert!(created.token.starts_with("rsdm_a2a_"));

        let list = api_a2a_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = serde_json::to_string(&list.0).unwrap();
        assert!(body.contains("my other instance"), "{body}");
        assert!(
            !body.contains(&created.token),
            "listing must never carry the token"
        );

        let revoked =
            api_a2a_keys_revoke(State(test_state(dir.path())), Path("laptop".to_string()))
                .await
                .unwrap();
        assert!(revoked.revoked);
        let after = api_a2a_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        assert!(after.keys.is_empty());
    }

    #[tokio::test]
    async fn invalid_name_is_bad_request_and_unknown_revoke_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "Bad Name".to_string(),
                description: None,
            }),
        )
        .await
        else {
            panic!("invalid name should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let Err((revoke_status, _)) =
            api_a2a_keys_revoke(State(test_state(dir.path())), Path("nope".to_string())).await
        else {
            panic!("unknown key revoke should fail");
        };
        assert_eq!(revoke_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_name_is_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let _first = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let Err((status, _)) = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        else {
            panic!("duplicate name should be rejected");
        };
        assert_eq!(status, StatusCode::CONFLICT);
    }

    // ── Remote agents ────────────────────────────────────────────────

    #[tokio::test]
    async fn agents_list_reports_status_and_card() {
        let hub = crate::a2a::A2aClientHub::new_shared();
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            crate::a2a::AgentSource::Config,
        )
        .await;
        let Json(agents) = api_a2a_agents_list(State(A2aAgentsStatusState {
            hub: Arc::clone(&hub),
        }))
        .await;
        let [only] = agents.as_slice() else {
            panic!("expected exactly one agent");
        };
        assert_eq!(only.name, "laptop");
        assert_eq!(
            only.status, "error",
            "an unreachable agent reports error status"
        );
        assert!(only.error.is_some());
        assert!(only.card.is_none());
    }

    #[tokio::test]
    async fn agents_raw_get_returns_default_template_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resp = api_a2a_agents_raw_get(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), DEFAULT_A2A_AGENTS_JSON.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_writes_file_and_get_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"laptop":{"url":"https://laptop.example.com"}}}"#;
        let result = api_a2a_agents_raw_put(State(test_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(result.valid);

        let resp = api_a2a_agents_raw_get(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), content.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_rejects_invalid_json_with_plain_language_error() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, Json(result))) =
            api_a2a_agents_raw_put(State(test_state(dir.path())), "not json".to_string()).await
        else {
            panic!("invalid JSON should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!result.valid);
        assert!(result.error.unwrap().contains("invalid JSON"));
    }

    #[tokio::test]
    async fn agents_raw_put_rejects_bad_agent_name() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"Bad Name":{"url":"https://x.example.com"}}}"#;
        let Err((status, Json(result))) =
            api_a2a_agents_raw_put(State(test_state(dir.path())), content.to_string()).await
        else {
            panic!("bad agent name should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!result.valid);
    }
}
