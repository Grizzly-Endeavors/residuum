use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use residuum_config::secrets::{SecretStore, is_reference};

use super::HubApiState;

#[derive(Deserialize)]
pub(super) struct SetSecretRequest {
    pub name: String,
    pub value: String,
}

#[derive(Serialize)]
pub(super) struct SetSecretResponse {
    pub reference: String,
}

#[derive(Serialize)]
pub(super) struct ListSecretsResponse {
    pub names: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct DeleteSecretResponse {
    pub deleted: bool,
}

pub(super) async fn api_secrets_set(
    State(state): State<HubApiState>,
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
    state
        .checkpoint_config_before_write(format!("set secret '{}'", req.name))
        .await;

    let hub_dir = state.hub_dir.clone();
    let name = req.name;
    let value = req.value;

    residuum_core::util::spawn_blocking_in_span(move || {
        let mut store = SecretStore::load(&hub_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load secret store: {e}"),
            )
        })?;
        store.set(&name, &value, &hub_dir).map_err(|e| {
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

pub(super) async fn api_secrets_list(
    State(state): State<HubApiState>,
) -> Result<Json<ListSecretsResponse>, (StatusCode, String)> {
    let hub_dir = state.hub_dir.clone();

    residuum_core::util::spawn_blocking_in_span(move || {
        let store = SecretStore::load(&hub_dir).map_err(|e| {
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

pub(super) async fn api_secrets_delete(
    State(state): State<HubApiState>,
    Path(name): Path<String>,
) -> Result<Json<DeleteSecretResponse>, (StatusCode, String)> {
    state
        .checkpoint_config_before_write(format!("delete secret '{name}'"))
        .await;
    let hub_dir = state.hub_dir.clone();

    residuum_core::util::spawn_blocking_in_span(move || {
        let mut store = SecretStore::load(&hub_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load secret store: {e}"),
            )
        })?;
        store.delete(&name, &hub_dir).map_err(|e| {
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
    use crate::web::HubApiState;

    fn test_state(dir: &std::path::Path) -> HubApiState {
        HubApiState::for_test(dir, crate::testing::fixture_assets)
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
