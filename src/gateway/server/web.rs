//! Web UI static asset serving and config API endpoints.
//!
//! Embeds the `assets/web/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Json, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::Config;
use crate::memory::recent_messages::load_recent_messages;

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
    /// Path to the workspace memory directory (None in setup mode).
    pub memory_dir: Option<PathBuf>,
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
        .route("/api/chat/history", get(api_chat_history))
        .route("/api/providers/models", post(api_provider_models))
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

/// `GET /api/chat/history` — return recent messages for the chat feed.
///
/// Reads `recent_messages.json` from the workspace memory directory.
/// Returns an empty array in setup mode or when the file doesn't exist.
async fn api_chat_history(
    State(state): State<ConfigApiState>,
) -> Json<Vec<crate::memory::recent_messages::RecentMessage>> {
    let Some(memory_dir) = &state.memory_dir else {
        return Json(Vec::new());
    };

    let path = memory_dir.join("recent_messages.json");
    match load_recent_messages(&path).await {
        Ok(messages) => Json(messages),
        Err(err) => {
            tracing::debug!(error = %err, "no chat history available");
            Json(Vec::new())
        }
    }
}

// ── Provider model listing ────────────────────────────────────────────

/// Request body for `POST /api/providers/models`.
#[derive(Deserialize)]
struct ModelsRequest {
    provider: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// A single model entry returned by the listing endpoint.
#[derive(Serialize)]
struct ModelEntry {
    id: String,
    name: String,
}

/// Response from the model listing endpoint.
#[derive(Serialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/providers/models` — fetch available models from a provider API.
///
/// Used by the setup wizard and settings page to populate model dropdowns.
/// Takes provider type, optional API key, and optional base URL.
async fn api_provider_models(Json(req): Json<ModelsRequest>) -> Json<ModelsResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let result = match req.provider.as_str() {
        "anthropic" => {
            fetch_anthropic_models(&client, req.api_key.as_deref(), req.url.as_deref()).await
        }
        "openai" => fetch_openai_models(&client, req.api_key.as_deref(), req.url.as_deref()).await,
        "gemini" => fetch_gemini_models(&client, req.api_key.as_deref(), req.url.as_deref()).await,
        "ollama" => fetch_ollama_models(&client, req.url.as_deref()).await,
        other => Err(format!("unknown provider: {other}")),
    };

    match result {
        Ok(mut models) => {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            Json(ModelsResponse {
                models,
                error: None,
            })
        }
        Err(err) => Json(ModelsResponse {
            models: Vec::new(),
            error: Some(err),
        }),
    }
}

/// Fetch models from Anthropic's `/v1/models` endpoint.
async fn fetch_anthropic_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for anthropic")?;
    let base = base_url.unwrap_or("https://api.anthropic.com");
    let url = format!("{base}/v1/models?limit=1000");

    let resp = client
        .get(&url)
        .header("X-Api-Key", key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("anthropic returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array")?;

    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let name = m
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| m.get("id").and_then(|v| v.as_str()).unwrap_or(""))
                .to_string();
            Some(ModelEntry { id, name })
        })
        .collect())
}

/// Fetch models from the `OpenAI` `/models` endpoint.
async fn fetch_openai_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for openai")?;
    let base = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{base}/models");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("openai returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array")?;

    let skip_prefixes = [
        "ft:",
        "dall-e",
        "tts-",
        "whisper",
        "text-embedding",
        "babbage",
        "davinci",
    ];

    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            if skip_prefixes.iter().any(|prefix| id.starts_with(prefix)) {
                return None;
            }
            Some(ModelEntry {
                id: id.to_string(),
                name: id.to_string(),
            })
        })
        .collect())
}

/// Fetch models from Google Gemini's `/models` endpoint.
async fn fetch_gemini_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for gemini")?;
    let base = base_url.unwrap_or("https://generativelanguage.googleapis.com/v1beta");
    let url = format!("{base}/models?key={key}&pageSize=1000");

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("gemini returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let models = json
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("missing models array")?;

    Ok(models
        .iter()
        .filter_map(|m| {
            // Only include models that support generateContent
            let methods = m
                .get("supportedGenerationMethods")
                .and_then(|v| v.as_array())?;
            let supports_generate = methods
                .iter()
                .any(|method| method.as_str().is_some_and(|s| s == "generateContent"));
            if !supports_generate {
                return None;
            }

            let raw_name = m.get("name")?.as_str()?;
            let id = raw_name
                .strip_prefix("models/")
                .unwrap_or(raw_name)
                .to_string();
            let display = m
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            Some(ModelEntry { id, name: display })
        })
        .collect())
}

/// Fetch models from Ollama's `/api/tags` endpoint.
async fn fetch_ollama_models(
    client: &reqwest::Client,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let base = base_url.unwrap_or("http://localhost:11434");
    let url = format!("{base}/api/tags");

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("ollama returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let models = json
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("missing models array")?;

    Ok(models
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            Some(ModelEntry {
                id: name.clone(),
                name,
            })
        })
        .collect())
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

    #[tokio::test]
    async fn chat_history_returns_empty_when_no_memory_dir() {
        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/ironclaw-test-nonexistent"),
            memory_dir: None,
            reload_sender: None,
            setup_done: None,
        };
        let Json(messages) = api_chat_history(State(state)).await;
        assert!(
            messages.is_empty(),
            "setup mode should return empty history"
        );
    }

    #[tokio::test]
    async fn chat_history_returns_empty_when_file_missing() {
        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/ironclaw-test-nonexistent"),
            memory_dir: Some(PathBuf::from("/tmp/ironclaw-test-nonexistent-memory")),
            reload_sender: None,
            setup_done: None,
        };
        let Json(messages) = api_chat_history(State(state)).await;
        assert!(
            messages.is_empty(),
            "missing file should return empty history"
        );
    }
}
