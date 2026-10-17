//! A2A web API endpoints the hub serves: caller-key management
//! (`/api/hub/a2a/keys`) and the `config/a2a.json` editor
//! (`GET`/`PUT /api/agents/{name}/a2a/agents/raw`), a file route that
//! answers whether or not the agent is running.
//!
//! Each request opens its own handle on the key store; writes are serialized
//! across handles and processes by the store's file lock, so the web UI,
//! the CLI, and the running A2A listener never lose each other's updates.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use residuum_a2a::{A2aKeyError, A2aKeyInfo, A2aKeys};

use super::config::ValidateResponse;
use super::{AgentFilesState, HubApiState};

/// Request body for `POST /api/hub/a2a/keys`.
#[derive(Deserialize)]
pub(super) struct CreateA2aKeyRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response from `POST /api/hub/a2a/keys`. Carries the token — shown only here,
/// once, and never again.
#[derive(Serialize)]
pub(super) struct CreateA2aKeyResponse {
    pub name: String,
    pub token: String,
}

/// Response from `GET /api/hub/a2a/keys`.
#[derive(Serialize)]
pub(super) struct ListA2aKeysResponse {
    pub keys: Vec<A2aKeyInfo>,
}

/// Response from `DELETE /api/hub/a2a/keys/{name}`.
#[derive(Serialize)]
pub(super) struct RevokeA2aKeyResponse {
    pub revoked: bool,
    /// Checkpoint holding the caller-key store as it was before this revoke.
    /// `None` when that checkpoint could not be recorded; the revoke still
    /// succeeded, and the UI should not offer Undo.
    pub checkpoint_id: Option<String>,
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

/// `GET /api/hub/a2a/keys` — list caller keys (metadata only, never tokens).
pub(super) async fn api_a2a_keys_list(
    State(state): State<HubApiState>,
) -> Result<Json<ListA2aKeysResponse>, (StatusCode, String)> {
    let snapshot = A2aKeys::new(state.hub_dir)
        .snapshot()
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(ListA2aKeysResponse {
        keys: snapshot.list(),
    }))
}

/// `POST /api/hub/a2a/keys` — mint a caller key, returning the token once.
pub(super) async fn api_a2a_keys_create(
    State(state): State<HubApiState>,
    Json(req): Json<CreateA2aKeyRequest>,
) -> Result<Json<CreateA2aKeyResponse>, (StatusCode, String)> {
    state
        .checkpoint_config_before_write(format!("create a2a key '{}'", req.name))
        .await;
    let token = A2aKeys::new(state.hub_dir)
        .create(&req.name, req.description.as_deref())
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(CreateA2aKeyResponse {
        name: req.name,
        token,
    }))
}

/// `DELETE /api/hub/a2a/keys/{name}` — revoke a caller key.
pub(super) async fn api_a2a_keys_revoke(
    State(state): State<HubApiState>,
    Path(name): Path<String>,
) -> Result<Json<RevokeA2aKeyResponse>, (StatusCode, String)> {
    let checkpoint_id = state
        .checkpoint_config_id_before_write(format!("revoke a2a key '{name}'"))
        .await;
    A2aKeys::new(state.hub_dir)
        .revoke(&name)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(RevokeA2aKeyResponse {
        revoked: true,
        checkpoint_id,
    }))
}

/// Default `config/a2a.json` content when the file doesn't exist yet.
const DEFAULT_A2A_AGENTS_JSON: &str = r#"{"agents":{}}"#;

/// `GET /api/agents/{name}/a2a/agents/raw` — return raw `config/a2a.json` contents.
pub(super) async fn api_a2a_agents_raw_get(
    State(state): State<AgentFilesState>,
) -> Result<Response, (StatusCode, String)> {
    let path = residuum_config::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
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

/// `PUT /api/agents/{name}/a2a/agents/raw` — write `config/a2a.json` atomically, save
/// unconditionally, trigger a workspace reload, and report diagnostics.
///
/// The save always succeeds, even when `body` fails validation: the loader
/// skips an unusable agent entry with a warning and keeps every other agent
/// running (see `residuum_a2a::client::config::load_a2a_agents_map`), so an
/// invalid save is safe to accept and report rather than reject outright —
/// consistent with `write_file`/`edit_file`, `config.toml`/`providers.toml`,
/// and the workspace file editor.
pub(super) async fn api_a2a_agents_raw_put(
    State(state): State<AgentFilesState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let diagnostics = residuum_a2a::client::config::diagnose_a2a_json(&body);

    let path = residuum_config::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    state
        .checkpoint_workspace_before_write("raw write a2a.json")
        .await;

    residuum_core::util::fs::atomic_write(&path, body.as_bytes())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write a2a.json: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx
            .send(residuum_bus::control::ReloadSignal::Workspace)
            .ok();
    }

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "a2a.json",
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use residuum_config::WorkspaceLayout;

    fn hub_state(dir: &std::path::Path) -> HubApiState {
        HubApiState::for_test(dir, crate::testing::fixture_assets)
    }

    fn files_state(dir: &std::path::Path) -> AgentFilesState {
        AgentFilesState::for_test(dir.join("workspace"), None)
    }

    #[tokio::test]
    async fn create_list_revoke_roundtrip_returns_token_only_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let created = api_a2a_keys_create(
            State(hub_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: Some("my other instance".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(created.name, "laptop");
        assert!(created.token.starts_with("rsdm_a2a_"));

        let list = api_a2a_keys_list(State(hub_state(dir.path())))
            .await
            .unwrap();
        let body = serde_json::to_string(&list.0).unwrap();
        assert!(body.contains("my other instance"), "{body}");
        assert!(
            !body.contains(&created.token),
            "listing must never carry the token"
        );

        let revoked = api_a2a_keys_revoke(State(hub_state(dir.path())), Path("laptop".to_string()))
            .await
            .unwrap();
        assert!(revoked.revoked);
        let after = api_a2a_keys_list(State(hub_state(dir.path())))
            .await
            .unwrap();
        assert!(
            after.keys.is_empty(),
            "expected {:?} to be empty",
            after.keys
        );
    }

    #[tokio::test]
    async fn revoke_returns_the_checkpoint_taken_before_the_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::testing::watching_hub_state(dir.path());
        let _created = api_a2a_keys_create(
            State(state.clone()),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let revoked = api_a2a_keys_revoke(State(state.clone()), Path("laptop".to_string()))
            .await
            .unwrap();
        let id = revoked
            .checkpoint_id
            .clone()
            .expect("revoke should name the checkpoint taken before it");
        let stored = state
            .checkpoints
            .file_content_at(
                residuum_checkpoints::RepoKind::Hub,
                id.clone(),
                "a2a-keys.toml".to_string(),
            )
            .await
            .unwrap();
        assert!(
            stored.is_some(),
            "the returned checkpoint must still contain the caller-key file"
        );

        std::fs::write(state.hub_dir.join("config.toml"), "later = true").unwrap();
        let later = state
            .checkpoint_config_id_before_write("later config write")
            .await
            .expect("a later checkpoint should be recorded");
        assert_ne!(
            later, id,
            "the id returned for Undo stays the pre-revoke checkpoint after a newer one is taken"
        );
    }

    #[tokio::test]
    async fn invalid_name_is_bad_request_and_unknown_revoke_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_a2a_keys_create(
            State(hub_state(dir.path())),
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
            api_a2a_keys_revoke(State(hub_state(dir.path())), Path("nope".to_string())).await
        else {
            panic!("unknown key revoke should fail");
        };
        assert_eq!(revoke_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_name_is_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let _first = api_a2a_keys_create(
            State(hub_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let Err((status, _)) = api_a2a_keys_create(
            State(hub_state(dir.path())),
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

    #[tokio::test]
    async fn agents_raw_get_returns_default_template_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resp = api_a2a_agents_raw_get(State(files_state(dir.path())))
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
        let result = api_a2a_agents_raw_put(State(files_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(result.valid);

        let resp = api_a2a_agents_raw_get(State(files_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), content.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_saves_invalid_json_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let state = files_state(dir.path());
        let result = api_a2a_agents_raw_put(State(state.clone()), "not json".to_string())
            .await
            .unwrap();
        assert!(!result.valid, "invalid JSON should be flagged invalid");
        assert!(
            !result.diagnostics.is_empty(),
            "result.diagnostics should not be empty"
        );

        let path = WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
        let saved = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            saved, "not json",
            "the save should have happened despite the invalid content"
        );
    }

    /// A state whose checkpoint repositories can't be opened, counting the
    /// attempts.
    fn unopenable_checkpoints(
        dir: &std::path::Path,
    ) -> (AgentFilesState, Arc<std::sync::atomic::AtomicUsize>) {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&attempts);
        let mut state = files_state(dir);
        state.checkpoints = crate::web::CheckpointAccess::lazy(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(residuum_checkpoints::CheckpointError::Git(
                "the repository is corrupt".to_string(),
            ))
        });
        (state, attempts)
    }

    #[tokio::test]
    async fn agents_raw_put_opens_the_checkpoint_repositories_once_and_saves_without_them() {
        let dir = tempfile::tempdir().unwrap();
        let (state, attempts) = unopenable_checkpoints(dir.path());
        let content = r#"{"agents":{"laptop":{"url":"https://laptop.example.com"}}}"#;

        let result = api_a2a_agents_raw_put(State(state.clone()), content.to_string())
            .await
            .unwrap();

        assert!(result.valid);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        let path = WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), content);
    }

    #[tokio::test]
    async fn agents_raw_get_never_opens_the_checkpoint_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let (state, attempts) = unopenable_checkpoints(dir.path());

        let resp = api_a2a_agents_raw_get(State(state)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn agents_raw_put_reports_bad_agent_name_without_blocking_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"Bad Name":{"url":"https://x.example.com"}}}"#;
        let result = api_a2a_agents_raw_put(State(files_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(!result.valid, "a bad agent name should be flagged invalid");
        assert_eq!(result.diagnostics.len(), 1);
    }
}
