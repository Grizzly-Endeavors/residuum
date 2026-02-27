//! Web UI static asset serving and config API endpoints.
//!
//! Embeds the `assets/web/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Json, Response};
use axum::routing::{get, post, put};
use serde::Serialize;
use tokio::sync::watch;

use crate::config::Config;

mod embedded {
    //! Module boundary isolates `rust-embed` derive from clippy `same_name_method`.
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed derive generates get/iter methods that shadow trait methods"
    )]

    use rust_embed::Embed;

    /// Embedded web assets from `assets/web/`.
    #[derive(Embed)]
    #[folder = "assets/web/"]
    pub(super) struct WebAssets;
}
use embedded::WebAssets;

/// Shared state for the config API.
#[derive(Clone)]
pub(super) struct ConfigApiState {
    /// Path to the ironclaw config directory (`~/.ironclaw/`).
    pub config_dir: PathBuf,
    /// Signal the running gateway to reload (None in setup mode).
    pub reload_sender: Option<watch::Sender<bool>>,
    /// Signal the setup server that config is saved (None in running mode).
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
}

/// Status response indicating which mode the server is running in.
#[derive(Serialize)]
struct StatusResponse {
    mode: &'static str,
}

/// Build the config API router.
pub(super) fn config_api_router(state: ConfigApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(api_status))
        .route("/api/config/raw", get(api_config_raw_get))
        .route("/api/config/raw", put(api_config_raw_put))
        .route("/api/config/validate", post(api_config_validate))
        .route("/api/config/complete-setup", post(api_complete_setup))
        .route("/api/system/timezone", get(api_system_timezone))
        .route("/api/mcp-catalog", get(api_mcp_catalog))
        .with_state(state)
}

/// `GET /api/status` — returns `{"mode":"setup"}` or `{"mode":"running"}`.
async fn api_status(State(state): State<ConfigApiState>) -> Json<StatusResponse> {
    let mode = if state.setup_done.is_some() {
        "setup"
    } else {
        "running"
    };
    Json(StatusResponse { mode })
}

/// `GET /api/config/raw` — return raw `config.toml` contents as text.
async fn api_config_raw_get(
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let config_path = state.config_dir.join("config.toml");
    let contents = tokio::fs::read_to_string(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read config: {e}"),
        )
    })?;
    Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(contents))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("response build error: {e}"),
            )
        })
}

/// `PUT /api/config/raw` — write TOML body, validate, save, trigger reload if running.
async fn api_config_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    // Validate first
    if let Err(e) = Config::validate_toml(&body) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(e),
            }),
        ));
    }

    // Write the config
    let config_path = state.config_dir.join("config.toml");
    tokio::fs::write(&config_path, &body).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("failed to write config: {e}")),
            }),
        )
    })?;

    // Trigger reload if in running mode
    if let Some(reload_sender) = &state.reload_sender {
        reload_sender.send(true).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// Response from validation or save endpoints.
#[derive(Serialize)]
struct ValidateResponse {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/config/validate` — validate TOML body without saving.
async fn api_config_validate(body: String) -> Json<ValidateResponse> {
    match Config::validate_toml(&body) {
        Ok(()) => Json(ValidateResponse {
            valid: true,
            error: None,
        }),
        Err(e) => Json(ValidateResponse {
            valid: false,
            error: Some(e),
        }),
    }
}

/// `POST /api/config/complete-setup` — validate + write config, signal setup done.
async fn api_complete_setup(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    // Validate
    if let Err(e) = Config::validate_toml(&body) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(e),
            }),
        ));
    }

    // Write config
    let config_path = state.config_dir.join("config.toml");
    tokio::fs::write(&config_path, &body).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("failed to write config: {e}")),
            }),
        )
    })?;

    // Signal setup server to shut down
    if let Some(done_sender) = &state.setup_done {
        done_sender.send(true).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// Timezone detection response.
#[derive(Serialize)]
struct TimezoneResponse {
    timezone: String,
}

/// `GET /api/system/timezone` — auto-detect system timezone.
async fn api_system_timezone() -> Json<TimezoneResponse> {
    let tz = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
    Json(TimezoneResponse { timezone: tz })
}

/// `GET /api/mcp-catalog` — serve the embedded MCP catalog JSON.
async fn api_mcp_catalog() -> Response {
    match WebAssets::get("mcp-catalog.json") {
        Some(content) => Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(content.data.to_vec()))
            .unwrap_or_else(|_| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap_or_default()
            }),
        None => Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("[]"))
            .unwrap_or_default(),
    }
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
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .unwrap_or_default()
}

/// Serve an embedded file by path, returning `None` if it doesn't exist.
fn serve_embedded(path: &str) -> Option<Response> {
    let asset = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut resp = Response::new(Body::from(asset.data.to_vec()));
    if let Ok(val) = HeaderValue::from_str(mime.as_ref()) {
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
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().contains("html"),
            "content type should be html"
        );
    }

    #[test]
    fn serve_embedded_returns_css_content_type() {
        let resp = serve_embedded("style.css").unwrap();
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().contains("css"),
            "content type should be css"
        );
    }

    #[test]
    fn serve_embedded_returns_none_for_missing() {
        assert!(
            serve_embedded("does-not-exist.txt").is_none(),
            "missing file should return None"
        );
    }
}
