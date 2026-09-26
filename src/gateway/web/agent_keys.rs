//! Agent key management API endpoints.
//!
//! Each request opens its own handle on the store; writes are serialized
//! across handles and processes by the store's file lock, so the web UI,
//! the CLI, and the running agent never lose each other's updates.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::agent_keys::{AgentKeyError, AgentKeyInfo, AgentKeys, KeyCreator, env_var_for};

use super::ConfigApiState;

/// Request body for `POST /api/agent-keys`.
#[derive(Deserialize)]
pub(super) struct SetAgentKeyRequest {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response from `POST /api/agent-keys`.
#[derive(Serialize)]
pub(super) struct SetAgentKeyResponse {
    pub name: String,
    pub env_var: String,
    /// Present when the value is short enough that redaction by substring
    /// match becomes unreliable. The key is stored either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Response from `GET /api/agent-keys`.
#[derive(Serialize)]
pub(super) struct ListAgentKeysResponse {
    pub keys: Vec<AgentKeyInfo>,
}

/// Response from `DELETE /api/agent-keys/{name}`.
#[derive(Serialize)]
pub(super) struct DeleteAgentKeyResponse {
    pub deleted: bool,
    /// Checkpoint holding the key store as it was before this delete.
    /// `None` when that checkpoint could not be recorded; the delete still
    /// succeeded, and the UI should not offer Undo.
    pub checkpoint_id: Option<String>,
}

fn error_response(e: &AgentKeyError) -> (StatusCode, String) {
    let status = match e {
        AgentKeyError::Invalid(_) => StatusCode::BAD_REQUEST,
        AgentKeyError::NotFound(_) => StatusCode::NOT_FOUND,
        AgentKeyError::Storage(_) => {
            tracing::error!(error = %e, "agent key store request failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, e.to_string())
}

/// `GET /api/agent-keys` — list keys (metadata only, never values).
pub(super) async fn api_agent_keys_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<ListAgentKeysResponse>, (StatusCode, String)> {
    let snapshot = AgentKeys::new(state.config_dir)
        .snapshot()
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(ListAgentKeysResponse {
        keys: snapshot.store.list(),
    }))
}

/// `POST /api/agent-keys` — store a key as user-created.
pub(super) async fn api_agent_keys_set(
    State(state): State<ConfigApiState>,
    Json(req): Json<SetAgentKeyRequest>,
) -> Result<Json<SetAgentKeyResponse>, (StatusCode, String)> {
    state
        .checkpoint_config_before_write(format!("set agent key '{}'", req.name))
        .await;
    let warning = AgentKeys::new(state.config_dir)
        .set(
            &req.name,
            &req.value,
            req.description.as_deref(),
            KeyCreator::User,
        )
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(SetAgentKeyResponse {
        env_var: env_var_for(&req.name),
        name: req.name,
        warning,
    }))
}

/// `DELETE /api/agent-keys/{name}` — remove a key.
pub(super) async fn api_agent_keys_delete(
    State(state): State<ConfigApiState>,
    Path(name): Path<String>,
) -> Result<Json<DeleteAgentKeyResponse>, (StatusCode, String)> {
    let checkpoint_id = state
        .checkpoint_config_id_before_write(format!("delete agent key '{name}'"))
        .await;
    AgentKeys::new(state.config_dir)
        .delete(&name)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(DeleteAgentKeyResponse {
        deleted: true,
        checkpoint_id,
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
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    #[tokio::test]
    async fn set_list_delete_roundtrip_never_returns_values() {
        let dir = tempfile::tempdir().unwrap();
        let set = api_agent_keys_set(
            State(test_state(dir.path())),
            Json(SetAgentKeyRequest {
                name: "github_token".to_string(),
                value: "ghp_web_value_123".to_string(),
                description: Some("repo access".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(set.env_var, "GITHUB_TOKEN", "response names the env var");

        let list = api_agent_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = serde_json::to_string(&list.0).unwrap();
        assert!(
            body.contains("\"created_by\":\"user\"") && body.contains("repo access"),
            "listing carries metadata: {body}"
        );
        assert!(
            !body.contains("ghp_web_value_123"),
            "listing must never carry values"
        );

        let deleted = api_agent_keys_delete(
            State(test_state(dir.path())),
            Path("github_token".to_string()),
        )
        .await
        .unwrap();
        assert!(deleted.deleted, "delete should report success");
        let after = api_agent_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        assert!(after.keys.is_empty(), "key should be gone after delete");
    }

    #[tokio::test]
    async fn short_value_is_stored_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let set = api_agent_keys_set(
            State(test_state(dir.path())),
            Json(SetAgentKeyRequest {
                name: "short_key".to_string(),
                value: "short".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();
        assert!(
            set.warning.as_deref().is_some_and(|w| w.contains("redact")),
            "a short value should warn, not be rejected: {:?}",
            set.warning
        );

        let list = api_agent_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        assert!(
            list.keys.iter().any(|k| k.name == "short_key"),
            "the short-valued key should still be stored"
        );
    }

    #[tokio::test]
    async fn invalid_name_is_bad_request_and_unknown_delete_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_agent_keys_set(
            State(test_state(dir.path())),
            Json(SetAgentKeyRequest {
                name: "Bad Name".to_string(),
                value: "long-enough-value".to_string(),
                description: None,
            }),
        )
        .await
        else {
            panic!("invalid name should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST, "invalid name is a 400");

        let Err((delete_status, _)) =
            api_agent_keys_delete(State(test_state(dir.path())), Path("nope".to_string())).await
        else {
            panic!("unknown key delete should fail");
        };
        assert_eq!(delete_status, StatusCode::NOT_FOUND, "unknown key is a 404");
    }

    #[tokio::test]
    async fn delete_returns_the_checkpoint_taken_before_the_delete() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_state(dir.path());
        let _created = api_agent_keys_set(
            State(state.clone()),
            Json(SetAgentKeyRequest {
                name: "github_token".to_string(),
                value: "ghp_web_value_123".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let deleted = api_agent_keys_delete(State(state.clone()), Path("github_token".to_string()))
            .await
            .unwrap();
        let id = deleted
            .checkpoint_id
            .clone()
            .expect("delete should name the checkpoint taken before it");
        let stored = state
            .checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Config,
                id.clone(),
                "agent-keys.toml.enc".to_string(),
            )
            .await
            .unwrap();
        assert!(
            stored.is_some(),
            "the returned checkpoint must still contain the key file"
        );

        std::fs::write(state.config_dir.join("config.toml"), "later = true").unwrap();
        let later = state
            .checkpoint_config_id_before_write("later config write")
            .await
            .expect("a later checkpoint should be recorded");
        assert_ne!(
            later, id,
            "the id returned for Undo stays the pre-delete checkpoint after a newer one is taken"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_omits_checkpoint_id_when_checkpointing_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_state(dir.path());
        let _created = api_agent_keys_set(
            State(state.clone()),
            Json(SetAgentKeyRequest {
                name: "github_token".to_string(),
                value: "ghp_web_value_123".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let objects = dir
            .path()
            .join("checkpoints")
            .join("config.git")
            .join("objects");
        std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o500)).unwrap();
        let _reset = ResetObjects(&objects);

        let deleted = api_agent_keys_delete(State(state.clone()), Path("github_token".to_string()))
            .await
            .unwrap();
        assert!(
            deleted.deleted,
            "the delete still succeeds when the checkpoint fails"
        );
        assert!(
            deleted.checkpoint_id.is_none(),
            "a failed checkpoint must not hand Undo an id"
        );
        let after = api_agent_keys_list(State(state)).await.unwrap();
        assert!(
            after.keys.is_empty(),
            "the key is gone even though checkpointing failed"
        );
    }

    #[cfg(unix)]
    struct ResetObjects<'a>(&'a std::path::Path);

    #[cfg(unix)]
    impl Drop for ResetObjects<'_> {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _reset = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
        }
    }
}
