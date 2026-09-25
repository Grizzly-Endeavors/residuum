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
}

fn error_response(e: &AgentKeyError) -> (StatusCode, String) {
    let status = match e {
        AgentKeyError::Invalid(_) => StatusCode::BAD_REQUEST,
        AgentKeyError::NotFound(_) => StatusCode::NOT_FOUND,
        AgentKeyError::OwnedByUser(_) => StatusCode::CONFLICT,
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
    state
        .checkpoint_config_before_write(format!("delete agent key '{name}'"))
        .await;
    AgentKeys::new(state.config_dir)
        .delete(&name, KeyCreator::User)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(DeleteAgentKeyResponse { deleted: true }))
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
}
