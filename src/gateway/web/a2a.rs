//! A2A caller-key management API endpoints.
//!
//! Each request opens its own handle on the store; writes are serialized
//! across handles and processes by the store's file lock, so the web UI,
//! the CLI, and the running A2A listener never lose each other's updates.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::a2a::{A2aKeyError, A2aKeyInfo, A2aKeys};

use super::ConfigApiState;

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
}
