//! Config API endpoints and types.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::memory::episode_store::{latest_episode_id, previous_episode_id, read_episode_jsonl};
use crate::memory::recent_messages::{RecentMessage, load_recent_messages};
use crate::memory::types::Visibility;
use crate::models::Message;

use super::ConfigApiState;

/// Status response indicating which mode the server is running in.
#[derive(Serialize)]
pub(super) struct StatusResponse {
    mode: &'static str,
}

/// Response from validation or save endpoints.
#[derive(Serialize)]
pub(super) struct ValidateResponse {
    pub(super) valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

/// Timezone detection response.
#[derive(Serialize)]
pub(super) struct TimezoneResponse {
    timezone: String,
}

/// Request body for the complete-setup endpoint.
#[derive(Deserialize)]
pub(super) struct CompleteSetupRequest {
    /// Raw config.toml content.
    config: String,
    /// Raw providers.toml content.
    providers: String,
    /// Raw mcp.json content (optional, Claude Code format).
    #[serde(default)]
    mcp_json: Option<String>,
}

/// `GET /api/status` — returns `{"mode":"setup"}` or `{"mode":"running"}`.
pub(super) async fn api_status(State(state): State<ConfigApiState>) -> Json<StatusResponse> {
    let mode = if state.setup_done.is_some() {
        "setup"
    } else {
        "running"
    };
    Json(StatusResponse { mode })
}

/// `GET /api/config/raw` — return raw `config.toml` contents as text.
pub(super) async fn api_config_raw_get(
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
pub(super) async fn api_config_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    // Validate first (use real config dir so secret:name references are checked)
    if let Err(e) = Config::validate_toml(&body, &state.config_dir) {
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
    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Root).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `POST /api/config/validate` — validate TOML body without saving.
pub(super) async fn api_config_validate(
    State(state): State<ConfigApiState>,
    body: String,
) -> Json<ValidateResponse> {
    match Config::validate_toml(&body, &state.config_dir) {
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

/// `GET /api/mcp/raw` — return raw `mcp.json` contents as JSON.
///
/// Returns `{"mcpServers":{}}` if the file doesn't exist yet.
pub(super) async fn api_mcp_raw_get(
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let mcp_path = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).mcp_json();

    let contents = match tokio::fs::read_to_string(&mcp_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => r#"{"mcpServers":{}}"#.to_string(),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read mcp.json: {e}"),
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

/// `PUT /api/mcp/raw` — validate JSON and write `mcp.json`, trigger workspace reload.
pub(super) async fn api_mcp_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    // Validate JSON parse
    serde_json::from_str::<serde_json::Value>(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("invalid JSON: {e}")),
            }),
        )
    })?;

    let mcp_path = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).mcp_json();

    if let Some(parent) = mcp_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    tokio::fs::write(&mcp_path, &body).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("failed to write mcp.json: {e}")),
            }),
        )
    })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `POST /api/config/complete-setup` — write config + providers, signal setup done.
pub(super) async fn api_complete_setup(
    State(state): State<ConfigApiState>,
    Json(body): Json<CompleteSetupRequest>,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let err = |msg: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(msg),
            }),
        )
    };

    // Parse both files to validate structure
    let config_file = toml::from_str::<crate::config::deserialize::ConfigFile>(&body.config)
        .map_err(|e| err(format!("config.toml parse error: {e}")))?;
    let providers_file =
        toml::from_str::<crate::config::deserialize::ProvidersFile>(&body.providers)
            .map_err(|e| err(format!("providers.toml parse error: {e}")))?;

    // Validate together
    crate::config::resolve::from_file_and_env(
        Some(&config_file),
        Some(&providers_file),
        &state.config_dir,
    )
    .map_err(|e| err(format!("{e}")))?;

    // Write providers.toml first (config validation reads it from disk)
    let providers_path = state.config_dir.join("providers.toml");
    tokio::fs::write(&providers_path, &body.providers)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write providers.toml: {e}")),
                }),
            )
        })?;

    // Write config.toml
    let config_path = state.config_dir.join("config.toml");
    tokio::fs::write(&config_path, &body.config)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write config.toml: {e}")),
                }),
            )
        })?;

    // Write mcp.json if provided
    if let Some(ref mcp_json) = body.mcp_json {
        let mcp_path = state
            .config_dir
            .join("workspace")
            .join("config")
            .join("mcp.json");
        if let Some(parent) = mcp_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&mcp_path, mcp_json).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write mcp.json: {e}")),
                }),
            )
        })?;
    }

    // Signal setup server to shut down
    if let Some(done_sender) = &state.setup_done {
        done_sender.send(true).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `GET /api/system/timezone` — auto-detect system timezone.
pub(super) async fn api_system_timezone() -> Json<TimezoneResponse> {
    let tz = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
    Json(TimezoneResponse { timezone: tz })
}

/// `GET /api/mcp-catalog` — serve the embedded MCP catalog JSON.
pub(super) async fn api_mcp_catalog() -> Response {
    use axum::http::StatusCode;

    match super::WebAssets::get("mcp-catalog.json") {
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

/// Query parameters for `GET /api/chat/history`.
#[derive(Debug, Deserialize)]
pub(super) struct ChatHistoryQuery {
    /// If set, fetch this specific episode instead of the live recent messages.
    #[serde(default)]
    pub(super) episode: Option<String>,
}

/// One segment of chat history returned by `GET /api/chat/history`.
///
/// `next_cursor`, if present, is the episode ID the client should pass back
/// as `?episode=<id>` to load the next-older segment.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ChatHistorySegment {
    /// Live, uncompressed messages from `recent_messages.json`.
    Recent {
        messages: Vec<RecentMessage>,
        next_cursor: Option<String>,
    },
    /// A single archived episode, synthesized as `RecentMessage`s so the
    /// frontend can render them through the existing pipeline.
    Episode {
        episode_id: String,
        date: NaiveDate,
        context: String,
        messages: Vec<RecentMessage>,
        next_cursor: Option<String>,
    },
}

/// `GET /api/chat/history` — return a segment of chat history.
///
/// With no query params, returns the live `Recent` segment plus a cursor
/// pointing at the newest episode on disk (for the frontend's lazy-load).
///
/// With `?episode=ep-NNN`, returns that episode's transcript wrapped as
/// `RecentMessage`s plus a cursor to the next-older episode. Returns 404
/// when the episode does not exist.
pub(super) async fn api_chat_history(
    State(state): State<ConfigApiState>,
    Query(params): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistorySegment>, StatusCode> {
    let Some(memory_dir) = &state.memory_dir else {
        return Ok(Json(ChatHistorySegment::Recent {
            messages: Vec::new(),
            next_cursor: None,
        }));
    };
    let episodes_dir = memory_dir.join("episodes");

    match params.episode {
        None => {
            let recent_path = memory_dir.join("recent_messages.json");
            let messages = match load_recent_messages(&recent_path).await {
                Ok(messages) => messages,
                Err(err) => {
                    tracing::debug!(error = %err, "no recent messages available");
                    Vec::new()
                }
            };
            let next_cursor = latest_episode_id(&episodes_dir)
                .await
                .inspect_err(|err| {
                    tracing::warn!(error = %err, "failed to scan episodes directory");
                })
                .ok()
                .flatten();
            Ok(Json(ChatHistorySegment::Recent {
                messages,
                next_cursor,
            }))
        }
        Some(episode_id) => {
            let path = crate::memory::episode_store::find_episode_path(&episodes_dir, &episode_id)
                .map_err(|err| {
                    tracing::warn!(error = %err, episode = %episode_id, "failed to locate episode");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let Some(path) = path else {
                return Err(StatusCode::NOT_FOUND);
            };

            let (meta, raw_messages) = read_episode_jsonl(&path).await.map_err(|err| {
                tracing::warn!(error = %err, episode = %episode_id, "failed to read episode transcript");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

            let timestamp = meta.date.and_hms_opt(0, 0, 0).unwrap_or_default();
            let messages = raw_messages
                .into_iter()
                .map(|message| wrap_episode_message(message, timestamp, meta.context.clone()))
                .collect();

            let next_cursor = previous_episode_id(&episodes_dir, &meta.id)
                .await
                .inspect_err(|err| {
                    tracing::warn!(error = %err, "failed to walk to previous episode");
                })
                .ok()
                .flatten();

            Ok(Json(ChatHistorySegment::Episode {
                episode_id: meta.id,
                date: meta.date,
                context: meta.context,
                messages,
                next_cursor,
            }))
        }
    }
}

/// Synthesize a `RecentMessage` wrapper around a raw episode `Message`.
///
/// Episode JSONL stores only raw `Message` values, so we fabricate metadata
/// using the episode's date (at 00:00) and context. Visibility is always
/// `User` because the original per-message visibility was not recorded in
/// the transcript.
fn wrap_episode_message(
    message: Message,
    timestamp: chrono::NaiveDateTime,
    project_context: String,
) -> RecentMessage {
    RecentMessage {
        message,
        timestamp,
        project_context,
        visibility: Visibility::User,
    }
}
