//! Config API endpoints and types.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::agent::usage::{SessionUsageTotals, load_session_usage_totals};
use crate::config::Config;
use crate::features;
use crate::inference::Message;
use crate::memory::episode_store::{latest_episode_id, previous_episode_id, read_episode_jsonl};
use crate::memory::recent_messages::{RecentMessage, load_recent_messages};
use crate::memory::types::Visibility;
use crate::update;

use super::ConfigApiState;

/// Status response: which mode the server is running in, its version, and the
/// feature ids it supports. Workbench artifacts read the same information
/// from the injected SDK's `residuum.version` and `residuum.features`.
#[derive(Serialize)]
pub(super) struct StatusResponse {
    mode: &'static str,
    version: &'static str,
    features: &'static [&'static str],
    /// On-disk size and checkpoint count for each checkpoint repository, so
    /// growth is visible before the web UI's own checkpoints view lands.
    /// `null` for a repo whose stats couldn't be read just now.
    checkpoints: CheckpointsStatus,
}

/// Per-repository checkpoint stats shown in `/api/status`.
#[derive(Serialize)]
pub(super) struct CheckpointsStatus {
    workspace: Option<crate::checkpoints::RepoStats>,
    config: Option<crate::checkpoints::RepoStats>,
}

/// Response from validation or save endpoints.
///
/// `diagnostics` carries the full detail (severity, plain-language message,
/// and location) from `crate::diagnostics` for a raw save or a validate-only
/// call; `valid`/`error` are a summary kept for callers that only check
/// whether saving is safe (`valid` is `false` if any diagnostic is an
/// error). A raw save always writes the file regardless of `valid` — see
/// `api_config_raw_put`/`api_providers_raw_put`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct ValidateResponse {
    pub(super) valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

impl ValidateResponse {
    /// Build a response from a set of diagnostics: `valid` is `false` if any
    /// are errors, and `error` summarizes the first one for callers that
    /// only look at the single-string field.
    pub(super) fn from_diagnostics(
        diagnostics: Vec<crate::diagnostics::Diagnostic>,
        file_label: &str,
    ) -> Self {
        let valid = !diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostics::Severity::Error);
        let error = diagnostics.first().map(|d| d.display(file_label));
        Self {
            valid,
            error,
            diagnostics,
        }
    }
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

/// `GET /api/status` — returns `{ mode, version, features, checkpoints }`.
pub(super) async fn api_status(State(state): State<ConfigApiState>) -> Json<StatusResponse> {
    let mode = if state.setup_done.is_some() {
        "setup"
    } else {
        "running"
    };
    Json(StatusResponse {
        mode,
        version: update::CURRENT_VERSION,
        features: features::FEATURES,
        checkpoints: CheckpointsStatus {
            workspace: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Workspace)
                .await,
            config: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Config).await,
        },
    })
}

/// A repo's checkpoint stats, or `None` (logged) if they couldn't be read —
/// `/api/status` degrades rather than failing over a checkpoint read.
async fn checkpoint_stats_or_log(
    state: &ConfigApiState,
    kind: crate::checkpoints::RepoKind,
) -> Option<crate::checkpoints::RepoStats> {
    state.checkpoints.stats(kind).await.map_or_else(
        |e| {
            tracing::warn!(error = %e, ?kind, "failed to read checkpoint stats for /api/status");
            None
        },
        Some,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn state(
        setup_done: Option<std::sync::Arc<tokio::sync::watch::Sender<bool>>>,
    ) -> ConfigApiState {
        ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            memory_dir: None,
            reload_tx: None,
            setup_done,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    #[tokio::test]
    async fn status_reports_mode_version_and_features() {
        let Json(running) = api_status(State(state(None))).await;
        assert_eq!(running.mode, "running");
        assert_eq!(running.version, update::CURRENT_VERSION);
        assert_eq!(running.features, features::FEATURES);

        let (tx, _rx) = tokio::sync::watch::channel(false);
        let Json(setup) = api_status(State(state(Some(std::sync::Arc::new(tx))))).await;
        assert_eq!(setup.mode, "setup");
    }

    /// A state backed by a real temp directory, for tests that read/write
    /// `config.toml`/`providers.toml` on disk.
    fn tempdir_state(dir: &std::path::Path) -> ConfigApiState {
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
    async fn config_raw_put_saves_invalid_toml_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let state = tempdir_state(dir.path());

        let Json(response) = api_config_raw_put(State(state), "this is not valid toml".to_string())
            .await
            .unwrap();

        assert!(!response.valid, "invalid TOML should be flagged invalid");
        assert!(
            !response.diagnostics.is_empty(),
            "invalid TOML should produce a diagnostic"
        );
        let saved = tokio::fs::read_to_string(dir.path().join("config.toml"))
            .await
            .unwrap();
        assert_eq!(
            saved, "this is not valid toml",
            "the save should have happened despite the invalid content"
        );
    }

    #[tokio::test]
    async fn config_raw_put_saves_valid_toml_with_no_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let state = tempdir_state(dir.path());

        let Json(response) = api_config_raw_put(State(state), "timezone = \"UTC\"\n".to_string())
            .await
            .unwrap();

        assert!(response.valid);
        assert!(response.diagnostics.is_empty());
    }
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

/// `PUT /api/config/raw` — write TOML body, save unconditionally, trigger
/// reload if running, and report diagnostics.
///
/// The save always succeeds, even when `body` fails validation: a config
/// reload that can't load the new file keeps the gateway running on its
/// current config and publishes a notice (see `handle_root_reload`), so an
/// invalid save is safe to accept and report rather than reject outright —
/// consistent with `write_file`/`edit_file` and the workspace file editor.
pub(super) async fn api_config_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    // Diagnose first (use real config dir so secret:name references are
    // checked), but never block the write on what it finds.
    let diagnostics = Config::diagnose_toml(&body, &state.config_dir);

    let config_path = state.config_dir.join("config.toml");
    state
        .checkpoint_config_before_write("raw write config.toml")
        .await;
    tokio::fs::write(&config_path, &body).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("failed to write config: {e}")),
                diagnostics: Vec::new(),
            }),
        )
    })?;

    // Trigger reload if in running mode
    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Root).ok();
    }

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "config.toml",
    )))
}

/// `PATCH /api/config/patch` — merge a JSON diff into the existing
/// `config.toml`, validate, save, trigger reload if running.
///
/// The diff's shape mirrors `config.toml`'s section/key layout, carrying
/// only the fields the Settings form actually changed — see
/// `crate::config::patch` for the exact convention. Everything the diff
/// doesn't mention (comments, unmodeled sections and keys) survives
/// untouched.
pub(super) async fn api_config_patch(
    State(state): State<ConfigApiState>,
    Json(diff): Json<serde_json::Value>,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let bad_request = |msg: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(msg),
                diagnostics: Vec::new(),
            }),
        )
    };

    let Some(diff_map) = diff.as_object() else {
        return Err(bad_request(
            "config patch must be a JSON object".to_string(),
        ));
    };

    let config_path = state.config_dir.join("config.toml");
    let existing = match tokio::fs::read_to_string(&config_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!(error = %e, path = %config_path.display(), "failed to read config.toml for patching");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to read config.toml: {e}")),
                    diagnostics: Vec::new(),
                }),
            ));
        }
    };

    let patched = crate::config::patch::apply_patch(&existing, diff_map, "config.toml").map_err(|msg| {
        tracing::warn!(error = %msg, path = %config_path.display(), "config.toml patch rejected");
        bad_request(msg)
    })?;

    Config::validate_toml(&patched, &state.config_dir).map_err(|e| {
        tracing::warn!(error = %e, path = %config_path.display(), "patched config.toml failed validation");
        bad_request(e)
    })?;

    state
        .checkpoint_config_before_write("patch config.toml")
        .await;
    crate::util::fs::atomic_write(&config_path, &patched)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, path = %config_path.display(), "failed to write patched config.toml");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write config.toml: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Root).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
        diagnostics: Vec::new(),
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
            diagnostics: Vec::new(),
        }),
        Err(e) => Json(ValidateResponse {
            valid: false,
            error: Some(e),
            diagnostics: Vec::new(),
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
                diagnostics: Vec::new(),
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
                diagnostics: Vec::new(),
            }),
        )
    })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
        diagnostics: Vec::new(),
    }))
}

/// `PATCH /api/mcp/patch` — merge a JSON diff into the existing `mcp.json`.
///
/// The diff's shape mirrors `mcp.json`: `{"mcpServers": {"<name>": {...}}}`.
/// Only the fields the Settings form actually changed need to be present —
/// see `crate::workspace::mcp_patch` for the exact convention. A server (or
/// field) the diff doesn't mention survives untouched, including fields the
/// form doesn't model (e.g. HTTP transport's `url`/`type`/`headers` when an
/// unrelated server is edited).
pub(super) async fn api_mcp_patch(
    State(state): State<ConfigApiState>,
    Json(diff): Json<serde_json::Value>,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let bad_request = |msg: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(msg),
                diagnostics: Vec::new(),
            }),
        )
    };

    let mcp_path = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).mcp_json();
    let existing = match tokio::fs::read_to_string(&mcp_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!(error = %e, path = %mcp_path.display(), "failed to read mcp.json for patching");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to read mcp.json: {e}")),
                    diagnostics: Vec::new(),
                }),
            ));
        }
    };

    let patched =
        crate::workspace::mcp_patch::apply_mcp_patch(&existing, &diff).map_err(|msg| {
            tracing::warn!(error = %msg, path = %mcp_path.display(), "mcp.json patch rejected");
            bad_request(msg)
        })?;

    if let Some(parent) = mcp_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    crate::util::fs::atomic_write(&mcp_path, &patched)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, path = %mcp_path.display(), "failed to write patched mcp.json");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write mcp.json: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
        diagnostics: Vec::new(),
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
                diagnostics: Vec::new(),
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
    state
        .checkpoint_config_before_write("complete setup: providers.toml + config.toml")
        .await;
    tokio::fs::write(&providers_path, &body.providers)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write providers.toml: {e}")),
                    diagnostics: Vec::new(),
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
                    diagnostics: Vec::new(),
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
                    diagnostics: Vec::new(),
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
        diagnostics: Vec::new(),
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
            let messages = load_recent_messages(&recent_path).await.map_err(|err| {
                tracing::warn!(
                    error = %err,
                    path = %recent_path.display(),
                    "failed to load recent messages — refusing to silently return empty history",
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            let next_cursor = latest_episode_id(&episodes_dir).await.map_err(|err| {
                tracing::warn!(
                    error = %err,
                    path = %episodes_dir.display(),
                    "failed to scan episodes directory",
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
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
                .map(|message| wrap_episode_message(message, timestamp))
                .collect();

            let next_cursor =
                previous_episode_id(&episodes_dir, &meta.id)
                    .await
                    .map_err(|err| {
                        tracing::warn!(
                            error = %err,
                            episode = %meta.id,
                            path = %episodes_dir.display(),
                            "failed to walk to previous episode",
                        );
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;

            Ok(Json(ChatHistorySegment::Episode {
                episode_id: meta.id,
                date: meta.date,
                messages,
                next_cursor,
            }))
        }
    }
}

/// `GET /api/usage` — the main agent's cumulative session token usage, for
/// the chat footer to render correctly on load or reconnect without
/// waiting for the next model call.
///
/// Reads the same on-disk totals the running agent writes through to after
/// every model call (see `crate::agent::usage::MainUsageSink`), the same
/// way `GET /api/chat/history` reads `recent_messages.json` rather than
/// reaching into the live agent — this HTTP layer never holds a reference
/// to it. Returns the zero default in setup mode (no memory dir yet).
pub(super) async fn api_usage(State(state): State<ConfigApiState>) -> Json<SessionUsageTotals> {
    let Some(memory_dir) = &state.memory_dir else {
        return Json(SessionUsageTotals::default());
    };
    let path = memory_dir.join("usage_totals.json");
    Json(load_session_usage_totals(&path).await)
}

/// Synthesize a `RecentMessage` wrapper around a raw episode `Message`.
///
/// Episode JSONL stores only raw `Message` values, so we fabricate metadata
/// using the episode's date (at 00:00). Visibility is always `User` because
/// the original per-message visibility was not recorded in the transcript.
fn wrap_episode_message(message: Message, timestamp: chrono::NaiveDateTime) -> RecentMessage {
    RecentMessage {
        message,
        timestamp,
        visibility: Visibility::User,
    }
}
