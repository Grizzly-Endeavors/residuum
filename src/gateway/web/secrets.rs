//! Secrets management API endpoints and types.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::config::secrets::{SecretStore, is_reference};

use super::ConfigApiState;

/// Request body for `POST /api/secrets`.
#[derive(Deserialize)]
pub(super) struct SetSecretRequest {
    pub name: String,
    pub value: String,
}

/// Response from `POST /api/secrets`.
#[derive(Serialize)]
pub(super) struct SetSecretResponse {
    pub reference: String,
}

/// Response from `GET /api/secrets`.
#[derive(Serialize)]
pub(super) struct ListSecretsResponse {
    pub names: Vec<String>,
}

/// Response from `DELETE /api/secrets/:name`.
#[derive(Serialize)]
pub(super) struct DeleteSecretResponse {
    pub deleted: bool,
}

/// `POST /api/secrets` — store a named secret in the encrypted store.
///
/// Rejects a value that is itself a reference (`secret:<name>` or
/// `${ENV_VAR}`) rather than a literal to store — storing a reference
/// verbatim would make the store return it unexpanded on lookup, handing
/// callers the literal token instead of a usable credential (see
/// `crate::config::resolve::resolve_secret_value`, which never re-expands a
/// stored value).
///
/// Acquires `secret_lock` to serialize concurrent writes and prevent
/// lost-update races (e.g. setup wizard storing multiple secrets via `Promise.all`).
pub(super) async fn api_secrets_set(
    State(state): State<ConfigApiState>,
    Json(req): Json<SetSecretRequest>,
) -> Result<Json<SetSecretResponse>, (StatusCode, String)> {
    if is_reference(&req.value) {
        tracing::warn!(
            name = %req.name,
            "refused to store a secret value that is itself a reference (a secret: prefix or an environment variable placeholder), not a literal to store"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            "that value looks like a secret reference (secret:<name> or a ${ENV_VAR} \
             placeholder), not a literal value — store the underlying credential instead"
                .to_string(),
        ));
    }

    let _guard = state.secret_lock.lock().await;

    let config_dir = state.config_dir.clone();
    let name = req.name;
    let value = req.value;

    tokio::task::spawn_blocking(move || {
        let mut store = SecretStore::load(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load secret store: {e}"),
            )
        })?;
        store.set(&name, &value, &config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to store secret: {e}"),
            )
        })?;
        Ok(Json(SetSecretResponse {
            reference: format!("secret:{name}"),
        }))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task join error: {e}"),
        )
    })?
}

/// `GET /api/secrets` — list stored secret names (not values).
pub(super) async fn api_secrets_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<ListSecretsResponse>, (StatusCode, String)> {
    let config_dir = state.config_dir.clone();

    tokio::task::spawn_blocking(move || {
        let store = SecretStore::load(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load secret store: {e}"),
            )
        })?;
        let names = store.names().into_iter().map(String::from).collect();
        Ok(Json(ListSecretsResponse { names }))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task join error: {e}"),
        )
    })?
}

/// `DELETE /api/secrets/{name}` — remove a named secret.
pub(super) async fn api_secrets_delete(
    State(state): State<ConfigApiState>,
    Path(name): Path<String>,
) -> Result<Json<DeleteSecretResponse>, (StatusCode, String)> {
    let config_dir = state.config_dir.clone();

    tokio::task::spawn_blocking(move || {
        let mut store = SecretStore::load(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load secret store: {e}"),
            )
        })?;
        store.delete(&name, &config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete secret: {e}"),
            )
        })?;
        Ok(Json(DeleteSecretResponse { deleted: true }))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task join error: {e}"),
        )
    })?
}

#[cfg(test)]
mod tests {
    use axum::Json;
    use axum::extract::State;

    use super::{SetSecretRequest, api_secrets_set};
    use crate::gateway::web::ConfigApiState;

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
    async fn rejects_a_secret_colon_reference_as_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let result = api_secrets_set(
            State(test_state(dir.path())),
            Json(SetSecretRequest {
                name: "fireworks".to_string(),
                value: "secret:other_name".to_string(),
            }),
        )
        .await;

        let Err((status, message)) = result else {
            panic!("expected a rejection, got a stored reference");
        };
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            message.contains("reference"),
            "error should explain the value looks like a reference: {message}"
        );
    }

    #[tokio::test]
    async fn rejects_an_env_var_reference_as_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let result = api_secrets_set(
            State(test_state(dir.path())),
            Json(SetSecretRequest {
                name: "fireworks".to_string(),
                value: "${FIREWORKS_API_KEY}".to_string(),
            }),
        )
        .await;

        let Err((status, _message)) = result else {
            panic!("expected a rejection, got a stored reference");
        };
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn accepts_a_literal_value() {
        let dir = tempfile::tempdir().unwrap();
        let result = api_secrets_set(
            State(test_state(dir.path())),
            Json(SetSecretRequest {
                name: "fireworks".to_string(),
                value: "sk-real-literal-key".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(result.0.reference, "secret:fireworks");
    }
}
