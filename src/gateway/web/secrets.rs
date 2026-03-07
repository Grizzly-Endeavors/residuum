//! Secrets management API endpoints and types.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::config::secrets::SecretStore;

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
/// Acquires `secret_lock` to serialize concurrent writes and prevent
/// lost-update races (e.g. setup wizard storing multiple secrets via `Promise.all`).
pub(super) async fn api_secrets_set(
	State(state): State<ConfigApiState>,
	Json(req): Json<SetSecretRequest>,
) -> Result<Json<SetSecretResponse>, (StatusCode, String)> {
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
