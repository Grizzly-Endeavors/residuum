//! Web UI static asset serving and config API endpoints.
//!
//! Embeds the `web/dist/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::Uri;
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use tokio::sync::watch;

use super::ReloadSignal;

pub mod cloud;
pub mod config;
pub mod providers;
pub mod secrets;

mod embedded {
    //! Module boundary isolates `rust-embed` derive from clippy `same_name_method`.
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed derive generates get/iter methods that shadow trait methods"
    )]

    use rust_embed::Embed;

    /// Embedded web assets from `web/dist/`.
    #[derive(Embed)]
    #[folder = "web/dist/"]
    pub(super) struct WebAssets;
}
use embedded::WebAssets;

/// Shared state for the config API.
#[derive(Clone)]
pub(crate) struct ConfigApiState {
    /// Path to the residuum config directory (`~/.residuum/`).
    pub config_dir: PathBuf,
    /// Path to the workspace root directory (for resolving `mcp.json`, `channels.toml`, etc.).
    pub workspace_dir: PathBuf,
    /// Path to the workspace memory directory (None in setup mode).
    pub memory_dir: Option<PathBuf>,
    /// Signal the running gateway to reload (None in setup mode).
    pub reload_tx: Option<watch::Sender<ReloadSignal>>,
    /// Signal the setup server that config is saved (None in running mode).
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
    /// Serializes secret store writes to prevent lost-update races.
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Build the config API router.
pub(super) fn config_api_router(state: ConfigApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(config::api_status))
        .route("/api/config/raw", get(config::api_config_raw_get))
        .route("/api/config/raw", put(config::api_config_raw_put))
        .route("/api/config/validate", post(config::api_config_validate))
        .route(
            "/api/config/complete-setup",
            post(config::api_complete_setup),
        )
        .route("/api/system/timezone", get(config::api_system_timezone))
        .route("/api/mcp-catalog", get(config::api_mcp_catalog))
        .route("/api/chat/history", get(config::api_chat_history))
        .route(
            "/api/providers/models",
            post(providers::api_provider_models),
        )
        .route("/api/providers/raw", get(config::api_providers_raw_get))
        .route("/api/providers/raw", put(config::api_providers_raw_put))
        .route(
            "/api/providers/validate",
            post(config::api_providers_validate),
        )
        .route("/api/mcp/raw", get(config::api_mcp_raw_get))
        .route("/api/mcp/raw", put(config::api_mcp_raw_put))
        .route("/api/secrets", post(secrets::api_secrets_set))
        .route("/api/secrets", get(secrets::api_secrets_list))
        .route("/api/secrets/{name}", delete(secrets::api_secrets_delete))
        .with_state(state)
}

/// Fallback handler for serving embedded static files.
///
/// Serves the file at the requested URI path, falling back to `index.html`
/// for SPA routing (paths without file extensions).
pub(super) async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first
    if let Some(resp) = serve_embedded(path) {
        return resp;
    }

    // SPA fallback: if no file extension, serve index.html
    if !path.contains('.')
        && let Some(resp) = serve_embedded("index.html")
    {
        return resp;
    }

    Response::builder()
        .status(axum::http::StatusCode::NOT_FOUND)
        .body(axum::body::Body::from("not found"))
        .unwrap_or_default()
}

/// Serve an embedded file by path, returning `None` if it doesn't exist.
fn serve_embedded(path: &str) -> Option<Response> {
    use axum::body::Body;
    use axum::http::header;

    let asset = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut resp = Response::new(Body::from(asset.data.to_vec()));
    if let Ok(val) = axum::http::HeaderValue::from_str(mime.as_ref()) {
        resp.headers_mut().insert(header::CONTENT_TYPE, val);
    }
    Some(resp)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn web_assets_contains_index_html() {
        assert!(
            WebAssets::get("index.html").is_some(),
            "index.html should be embedded"
        );
    }

    #[test]
    fn serve_embedded_returns_html_content_type() {
        let resp = serve_embedded("index.html").unwrap();
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap();
        assert!(
            ct.to_str().unwrap().contains("html"),
            "content type should be html"
        );
    }

    #[test]
    fn serve_embedded_returns_json_content_type() {
        let resp = serve_embedded("mcp-catalog.json").unwrap();
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap();
        assert!(
            ct.to_str().unwrap().contains("json"),
            "content type should be json"
        );
    }

    #[test]
    fn serve_embedded_returns_none_for_missing() {
        assert!(
            serve_embedded("does-not-exist.txt").is_none(),
            "missing file should return None"
        );
    }

    #[tokio::test]
    async fn chat_history_returns_empty_when_no_memory_dir() {
        use axum::Json;
        use axum::extract::State;

        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let Json(messages) = config::api_chat_history(State(state)).await;
        assert!(
            messages.is_empty(),
            "setup mode should return empty history"
        );
    }

    #[tokio::test]
    async fn chat_history_returns_empty_when_file_missing() {
        use axum::Json;
        use axum::extract::State;

        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            memory_dir: Some(PathBuf::from("/tmp/residuum-test-nonexistent-memory")),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let Json(messages) = config::api_chat_history(State(state)).await;
        assert!(
            messages.is_empty(),
            "missing file should return empty history"
        );
    }

    #[tokio::test]
    async fn secrets_set_list_delete_roundtrip() {
        use axum::Json;
        use axum::extract::{Path, State};

        let dir = tempfile::tempdir().unwrap();
        let state = ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
        };

        // Set a secret
        let set_result = secrets::api_secrets_set(
            State(state.clone()),
            Json(secrets::SetSecretRequest {
                name: "test_key".to_string(),
                value: "test_value".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(set_result.0.reference, "secret:test_key");

        // List secrets
        let list_result = secrets::api_secrets_list(State(state.clone()))
            .await
            .unwrap();
        assert_eq!(list_result.0.names, vec!["test_key"]);

        // Delete the secret
        let delete_result =
            secrets::api_secrets_delete(State(state.clone()), Path("test_key".to_string()))
                .await
                .unwrap();
        assert!(delete_result.0.deleted);

        // Verify it's gone
        let after_delete = secrets::api_secrets_list(State(state)).await.unwrap();
        assert!(after_delete.0.names.is_empty());
    }
}
