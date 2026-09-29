//! Config API endpoints and types.

use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use crate::features;
use crate::update;

use super::{ConfigApiState, HubApiState};

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

/// Per-repository checkpoint stats shown in `/api/agents/{name}/status`.
#[derive(Serialize)]
pub(super) struct CheckpointsStatus {
    workspace: Option<crate::checkpoints::RepoStats>,
    team: Option<crate::checkpoints::RepoStats>,
    agent_config: Option<crate::checkpoints::RepoStats>,
    hub: Option<crate::checkpoints::RepoStats>,
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

/// Success body for a Settings form PATCH (`config.toml`, `providers.toml`,
/// `mcp.json`): the validation summary plus the checkpoint taken just
/// before the write, so the web UI's Undo restores exactly this save.
/// `checkpoint_id` is absent when that checkpoint could not be recorded;
/// the write still went through, and the UI should not offer Undo.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct PatchSavedResponse {
    #[serde(flatten)]
    pub(super) validation: ValidateResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) checkpoint_id: Option<String>,
}

impl PatchSavedResponse {
    pub(super) fn saved(checkpoint_id: Option<String>) -> Self {
        Self {
            validation: ValidateResponse {
                valid: true,
                error: None,
                diagnostics: Vec::new(),
            },
            checkpoint_id,
        }
    }
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
    /// Raw `hub/config.toml` content.
    hub_config: String,
    /// This installation's first agent name, validated with
    /// [`crate::config::validate_agent_name`].
    agent_name: String,
    /// The user's name, written to the team's `USER.md`.
    #[serde(default)]
    user_name: Option<String>,
    /// Raw agent config.toml content.
    config: String,
    /// Raw providers.toml content.
    providers: String,
    /// Raw mcp.json content (optional, Claude Code format).
    #[serde(default)]
    mcp_json: Option<String>,
}

/// `GET /api/agents/{name}/status` — returns `{ mode, version, features, checkpoints }`.
pub(super) async fn api_status(State(state): State<ConfigApiState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        mode: "running",
        version: update::CURRENT_VERSION,
        features: features::FEATURES,
        checkpoints: CheckpointsStatus {
            workspace: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Workspace)
                .await,
            team: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Team).await,
            agent_config: checkpoint_stats_or_log(
                &state,
                crate::checkpoints::RepoKind::AgentConfig,
            )
            .await,
            hub: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Hub).await,
        },
    })
}

/// A repo's checkpoint stats, or `None` (logged) if they couldn't be read —
/// `/api/agents/{name}/status` degrades rather than failing over a checkpoint read.
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

/// `GET /api/hub/config/raw` — return raw `hub/config.toml` contents as text.
pub(super) async fn api_hub_config_raw_get(
    State(state): State<HubApiState>,
) -> Result<Response, (StatusCode, String)> {
    let config_path = state.hub_dir.join("config.toml");
    let contents = tokio::fs::read_to_string(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read hub config: {e}"),
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

/// `PUT /api/hub/config/raw` — write TOML body to `hub/config.toml`, save
/// unconditionally, trigger a hub reload if running, and report diagnostics.
///
/// Same "save regardless, report what's wrong" contract as
/// [`api_config_raw_put`] — a hub reload that can't load the new file keeps
/// the hub running on its last-known-good config (see
/// `gateway::reload::handle_hub_reload`).
pub(super) async fn api_hub_config_raw_put(
    State(state): State<HubApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let diagnostics = crate::config::HubConfig::diagnose_toml(&body, &state.hub_dir);

    let config_path = state.hub_dir.join("config.toml");
    state
        .checkpoint_config_before_write("raw write hub config.toml")
        .await;
    crate::util::fs::atomic_write(&config_path, &body)
        .await
        .map_err(|e| internal_error("write hub config", e))?;

    state.reload_tx.send(super::super::ReloadSignal::Hub).ok();

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "hub/config.toml",
    )))
}

/// `PATCH /api/hub/config/patch` — merge a JSON diff into the existing
/// `hub/config.toml`, validate, save, trigger a hub reload if running.
pub(super) async fn api_hub_config_patch(
    State(state): State<HubApiState>,
    Json(diff): Json<serde_json::Value>,
) -> Result<Json<PatchSavedResponse>, (StatusCode, Json<ValidateResponse>)> {
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
            "hub config patch must be a JSON object".to_string(),
        ));
    };

    let config_path = state.hub_dir.join("config.toml");
    let existing = match tokio::fs::read_to_string(&config_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!(error = %e, path = %config_path.display(), "failed to read hub config.toml for patching");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to read hub config.toml: {e}")),
                    diagnostics: Vec::new(),
                }),
            ));
        }
    };

    let patched =
        crate::config::patch::apply_patch(&existing, diff_map, "hub/config.toml").map_err(|msg| {
            tracing::warn!(error = %msg, path = %config_path.display(), "hub config.toml patch rejected");
            bad_request(msg)
        })?;

    crate::config::HubConfig::validate_toml(&patched, &state.hub_dir).map_err(|e| {
        tracing::warn!(error = %e, path = %config_path.display(), "patched hub config.toml failed validation");
        bad_request(e)
    })?;

    let checkpoint_id = state
        .checkpoint_config_id_before_write("patch hub config.toml")
        .await;
    crate::util::fs::atomic_write(&config_path, &patched)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, path = %config_path.display(), "failed to write patched hub config.toml");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write hub config.toml: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    state.reload_tx.send(super::super::ReloadSignal::Hub).ok();

    Ok(Json(PatchSavedResponse::saved(checkpoint_id)))
}

/// `POST /api/hub/config/validate` — validate hub config TOML body without saving.
pub(super) async fn api_hub_config_validate(
    State(state): State<HubApiState>,
    body: String,
) -> Json<ValidateResponse> {
    let diagnostics = crate::config::HubConfig::diagnose_toml(&body, &state.hub_dir);
    Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "hub/config.toml",
    ))
}

/// `GET /api/agents/{name}/config/raw` — return raw `config.toml` contents as text.
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

/// `PUT /api/agents/{name}/config/raw` — write TOML body, save unconditionally, trigger
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
    let diagnostics = state.diagnose_agent_config(&body);

    let config_path = state.config_dir.join("config.toml");
    state
        .checkpoint_agent_config_before_write("raw write config.toml")
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
        reload_tx.send(super::super::ReloadSignal::Agent).ok();
    }

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "config.toml",
    )))
}

/// `PATCH /api/agents/{name}/config/patch` — merge a JSON diff into the existing
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
) -> Result<Json<PatchSavedResponse>, (StatusCode, Json<ValidateResponse>)> {
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

    state.validate_agent_config(&patched).map_err(|e| {
        tracing::warn!(error = %e, path = %config_path.display(), "patched config.toml failed validation");
        bad_request(e)
    })?;

    let checkpoint_id = state
        .checkpoints
        .checkpoint_config_kind_id_before_write(
            crate::checkpoints::RepoKind::AgentConfig,
            crate::checkpoints::CheckpointContext::system(
                crate::checkpoints::CheckpointTrigger::PreConfigWrite,
                "patch config.toml",
            ),
        )
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
        reload_tx.send(super::super::ReloadSignal::Agent).ok();
    }

    Ok(Json(PatchSavedResponse::saved(checkpoint_id)))
}

/// `POST /api/agents/{name}/config/validate` — validate TOML body without saving.
pub(super) async fn api_config_validate(
    State(state): State<ConfigApiState>,
    body: String,
) -> Json<ValidateResponse> {
    let diagnostics = state.diagnose_agent_config(&body);
    Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "config.toml",
    ))
}

/// `GET /api/agents/{name}/mcp/raw` — return raw `mcp.json` contents as JSON.
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

/// `PUT /api/agents/{name}/mcp/raw` — write `mcp.json`, save unconditionally, trigger a
/// workspace reload, and report diagnostics.
///
/// The save always succeeds, even when `body` fails validation: the loader
/// skips an unusable server entry with a warning and keeps every other
/// server running (see `crate::workspace::config::load_mcp_servers_map`), so
/// an invalid save is safe to accept and report rather than reject outright
/// — consistent with `write_file`/`edit_file`, `config.toml`/`providers.toml`,
/// and the workspace file editor.
pub(super) async fn api_mcp_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let diagnostics = crate::workspace::config::diagnose_mcp_json(&body);

    let mcp_path = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).mcp_json();

    if let Some(parent) = mcp_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    state
        .checkpoint_workspace_before_write("raw write mcp.json")
        .await;

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

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "mcp.json",
    )))
}

/// `PATCH /api/agents/{name}/mcp/patch` — merge a JSON diff into the existing `mcp.json`.
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
) -> Result<Json<PatchSavedResponse>, (StatusCode, Json<ValidateResponse>)> {
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

    // `mcp.json` lives in the workspace, not the config repo's tracked-file
    // allowlist, and this write happens outside any agent turn — without
    // this it would never be checkpointed until the next turn boundary
    // happened to snapshot it as an "outside edit".
    let checkpoint_id = state
        .checkpoint_workspace_id_before_write("patch mcp.json")
        .await;

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

    Ok(Json(PatchSavedResponse::saved(checkpoint_id)))
}

/// A 500 response saying which step failed, in the shape every config
/// handler uses.
fn internal_error(action: &str, e: impl std::fmt::Display) -> (StatusCode, Json<ValidateResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ValidateResponse {
            valid: false,
            error: Some(format!("failed to {action}: {e:#}")),
            diagnostics: Vec::new(),
        }),
    )
}

/// Write the first agent's `providers.toml`, optional `mcp.json`, and
/// `config.toml` from the onboarding request, each atomically.
///
/// `config.toml` goes last: an agent is discovered by that file, so a
/// failure on an earlier write never leaves a discoverable half-configured
/// agent behind.
async fn write_first_agent_config_files(
    layout: &crate::workspace::layout::WorkspaceLayout,
    body: &CompleteSetupRequest,
) -> Result<(), (StatusCode, Json<ValidateResponse>)> {
    let agent_config_dir = layout.config_dir();
    crate::util::fs::atomic_write(&agent_config_dir.join("providers.toml"), &body.providers)
        .await
        .map_err(|e| internal_error("write providers.toml", e))?;

    if let Some(ref mcp_json) = body.mcp_json {
        let mcp_path = layout.mcp_json();
        if let Some(parent) = mcp_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| internal_error("create the mcp.json directory", e))?;
        }
        crate::util::fs::atomic_write(&mcp_path, mcp_json)
            .await
            .map_err(|e| internal_error("write mcp.json", e))?;
    }

    crate::util::fs::atomic_write(&agent_config_dir.join("config.toml"), &body.config)
        .await
        .map_err(|e| internal_error("write config.toml", e))?;
    Ok(())
}

/// Setup creates only the first agent: answer 409 when the residuum root
/// already holds one, so a live agent is never overwritten.
fn refuse_when_agents_exist(
    residuum_root: &std::path::Path,
    agent_name: &str,
) -> Result<(), (StatusCode, Json<ValidateResponse>)> {
    let existing = crate::config::discover_agents(residuum_root)
        .map_err(|e| internal_error("check for existing agents", e))?;
    if existing.is_empty() {
        return Ok(());
    }
    let message = if existing.iter().any(|name| name == agent_name) {
        format!(
            "An agent named '{agent_name}' already exists. Choose a different name, or change the existing agent from its settings."
        )
    } else {
        format!(
            "This residuum already has an agent ('{}'). Setup only creates the first agent.",
            existing.join("', '")
        )
    };
    Err((
        StatusCode::CONFLICT,
        Json(ValidateResponse {
            valid: false,
            error: Some(message),
            diagnostics: Vec::new(),
        }),
    ))
}

/// `POST /api/hub/config/complete-setup` — write config + providers, signal setup done.
///
/// Writes `hub/config.toml`, bootstraps the hub directory (`bin/`, `logs/`),
/// creates the first agent's directory (named `body.agent_name`, validated
/// with [`crate::config::validate_agent_name`]) under the residuum root,
/// bootstraps its full workspace (`SOUL.md`, bundled skills), the shared team
/// directory (`AGENTS.md`, the wiki, `USER.md` personalized with
/// `body.user_name`) and its role page, and writes its
/// `config.toml`/`providers.toml`/`mcp.json`, `config.toml` last.
///
/// Answers 409 when an agent already exists, so a running gateway's live
/// agent is never overwritten.
pub(super) async fn api_complete_setup(
    State(state): State<HubApiState>,
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

    crate::config::validate_agent_name(&body.agent_name).map_err(err)?;

    let residuum_root = state
        .hub_dir
        .parent()
        .map_or_else(|| state.hub_dir.clone(), std::path::Path::to_path_buf);
    let agent_dir = residuum_root.join(&body.agent_name);

    refuse_when_agents_exist(&residuum_root, &body.agent_name)?;

    // Parse and resolve the hub config on its own first — the agent config
    // resolves against it (timezone, gateway, ...).
    let hub_file = toml::from_str::<crate::config::deserialize::HubConfigFile>(&body.hub_config)
        .map_err(|e| err(format!("hub config.toml parse error: {e}")))?;
    let hub = crate::config::resolve::resolve_hub_config(Some(&hub_file), &state.hub_dir)
        .map_err(|e| err(format!("{e}")))?;

    let config_file = toml::from_str::<crate::config::deserialize::AgentConfigFile>(&body.config)
        .map_err(|e| err(format!("config.toml parse error: {e}")))?;
    let providers_file =
        toml::from_str::<crate::config::deserialize::ProvidersFile>(&body.providers)
            .map_err(|e| err(format!("providers.toml parse error: {e}")))?;

    // Validate the agent config together with the hub config and providers.
    crate::config::resolve::from_file_and_env(
        Some(&config_file),
        Some(&providers_file),
        &agent_dir,
        &body.agent_name,
        &hub,
    )
    .map_err(|e| err(format!("{e}")))?;

    // Write hub/config.toml, and bootstrap the hub directory (bin/, logs/).
    state
        .checkpoint_config_before_write("complete setup: hub config.toml")
        .await;
    crate::config::HubConfig::bootstrap_at(&state.hub_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResponse {
                valid: false,
                error: Some(format!("failed to bootstrap hub directory: {e}")),
                diagnostics: Vec::new(),
            }),
        )
    })?;
    crate::util::fs::atomic_write(&state.hub_dir.join("config.toml"), &body.hub_config)
        .await
        .map_err(|e| internal_error("write hub config.toml", e))?;

    // Bootstrap the agent's full workspace (identity files, wiki, bundled
    // skills), personalized with the user's name.
    let layout = crate::workspace::layout::WorkspaceLayout::new(&agent_dir);
    state
        .bootstrap_workspace(&layout, body.user_name.as_deref(), hub.timezone.name())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to bootstrap agent workspace: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    write_first_agent_config_files(&layout, &body).await?;

    // The hub picks up the new hub config, and whoever is waiting on setup
    // learns the first agent is on disk.
    state.reload_tx.send(super::super::ReloadSignal::Hub).ok();
    if let Some(done_sender) = &state.setup_done {
        done_sender.send(true).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
        diagnostics: Vec::new(),
    }))
}

/// `GET /api/hub/system/timezone` — auto-detect system timezone.
pub(super) async fn api_system_timezone() -> Json<TimezoneResponse> {
    let tz = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
    Json(TimezoneResponse { timezone: tz })
}

/// `GET /api/hub/mcp-catalog` — serve the embedded MCP catalog JSON.
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn state() -> ConfigApiState {
        ConfigApiState {
            team: None,
            hub_dir: PathBuf::from("/tmp/residuum-test-nonexistent-hub"),
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            agent_name: "test-agent".to_string(),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            memory_dir: None,
            reload_tx: None,
            scope: crate::gateway::web::WorkspaceScope::Agent,
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    #[tokio::test]
    async fn status_reports_mode_version_and_features() {
        let Json(running) = api_status(State(state())).await;
        assert_eq!(running.mode, "running");
        assert_eq!(running.version, update::CURRENT_VERSION);
        assert_eq!(running.features, features::FEATURES);
    }

    /// A state backed by a real temp directory, for tests that read/write
    /// `config.toml`/`providers.toml` on disk. Seeds a valid hub `config.toml`
    /// (just a timezone) so `diagnose_agent_config`/`validate_agent_config`
    /// can actually resolve the hub side rather than reporting "hub config
    /// couldn't be loaded" for every test.
    fn tempdir_state(dir: &std::path::Path) -> ConfigApiState {
        let hub_dir = dir.join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        ConfigApiState {
            team: None,
            hub_dir,
            config_dir: dir.to_path_buf(),
            agent_name: "test-agent".to_string(),
            workspace_dir: dir.join("workspace"),
            memory_dir: None,
            reload_tx: None,
            scope: crate::gateway::web::WorkspaceScope::Agent,
            checkpoints: crate::checkpoints::test_engine(),
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

        let Json(response) = api_config_raw_put(State(state), "max_tokens = 4096\n".to_string())
            .await
            .unwrap();

        assert!(response.valid);
        assert!(response.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn mcp_raw_put_saves_invalid_json_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempdir_state(dir.path());

        let Json(response) = api_mcp_raw_put(State(state), "not json".to_string())
            .await
            .unwrap();

        assert!(!response.valid, "invalid JSON should be flagged invalid");
        assert!(!response.diagnostics.is_empty());
        let mcp_path =
            crate::workspace::layout::WorkspaceLayout::new(dir.path().join("workspace")).mcp_json();
        let saved = tokio::fs::read_to_string(&mcp_path).await.unwrap();
        assert_eq!(
            saved, "not json",
            "the save should have happened despite the invalid content"
        );
    }

    #[tokio::test]
    async fn mcp_raw_put_saves_valid_json_with_no_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempdir_state(dir.path());

        let Json(response) = api_mcp_raw_put(
            State(state),
            r#"{"mcpServers":{"fs":{"command":"npx"}}}"#.to_string(),
        )
        .await
        .unwrap();

        assert!(response.valid);
        assert!(response.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn mcp_raw_put_reports_a_bad_server_entry_without_blocking_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempdir_state(dir.path());

        let Json(response) = api_mcp_raw_put(
            State(state),
            r#"{"mcpServers":{"broken":{"type":"sse"}}}"#.to_string(),
        )
        .await
        .unwrap();

        assert!(
            !response.valid,
            "a server with a deprecated transport should be flagged invalid"
        );
        assert_eq!(response.diagnostics.len(), 1);
    }

    #[tokio::test]
    async fn config_patch_returns_the_checkpoint_taken_before_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_state(dir.path());
        std::fs::create_dir_all(&state.hub_dir).unwrap();
        std::fs::write(state.hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        let before = "max_tokens = 4096\n";
        std::fs::write(state.config_dir.join("config.toml"), before).unwrap();

        let Json(saved) = api_config_patch(
            State(state.clone()),
            Json(serde_json::json!({"max_tokens": 8192})),
        )
        .await
        .unwrap();

        assert!(saved.validation.valid);
        let id = saved
            .checkpoint_id
            .expect("a patch should name the checkpoint taken before it");
        let stored = state
            .checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::AgentConfig,
                id,
                "config.toml".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(before.as_bytes()),
            "the returned checkpoint must hold config.toml as it was before the patch"
        );
    }

    #[tokio::test]
    async fn mcp_patch_returns_the_checkpoint_taken_before_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_state(dir.path());
        let mcp_path =
            crate::workspace::layout::WorkspaceLayout::new(state.workspace_dir.clone()).mcp_json();
        std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
        let before = r#"{"mcpServers":{"fs":{"command":"npx","cwd":"/srv"}}}"#;
        std::fs::write(&mcp_path, before).unwrap();

        let Json(saved) = api_mcp_patch(
            State(state.clone()),
            Json(serde_json::json!({"mcpServers": {"fs": null}})),
        )
        .await
        .unwrap();

        assert!(saved.validation.valid);
        let id = saved
            .checkpoint_id
            .expect("a patch should name the checkpoint taken before it");
        let relative = mcp_path
            .strip_prefix(&state.workspace_dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let stored = state
            .checkpoints
            .file_content_at(crate::checkpoints::RepoKind::Workspace, id, relative)
            .await
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(before.as_bytes()),
            "the returned checkpoint must still hold the removed server, including fields the form doesn't model"
        );
    }

    /// A setup-mode state rooted at `root`: the hub directory is `root/hub`,
    /// and no agent exists yet.
    fn setup_state(root: &std::path::Path) -> (HubApiState, tokio::sync::watch::Receiver<bool>) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut state = HubApiState::for_test(&root.join("hub"));
        state.setup_done = Some(std::sync::Arc::new(tx));
        (state, rx)
    }

    fn setup_request(agent_name: &str) -> CompleteSetupRequest {
        CompleteSetupRequest {
            hub_config: "timezone = \"UTC\"\n".to_string(),
            agent_name: agent_name.to_string(),
            user_name: Some("Sam".to_string()),
            config: String::new(),
            providers: "[models]\nmain = \"ollama/llama3\"\n".to_string(),
            mcp_json: None,
        }
    }

    #[tokio::test]
    async fn complete_setup_writes_hub_config_and_the_first_agent() {
        let root = tempfile::tempdir().unwrap();
        let (state, done_rx) = setup_state(root.path());

        let Json(response) = api_complete_setup(State(state), Json(setup_request("scout")))
            .await
            .unwrap();

        assert!(response.valid);
        assert_eq!(
            std::fs::read_to_string(root.path().join("hub/config.toml")).unwrap(),
            "timezone = \"UTC\"\n"
        );
        assert!(root.path().join("scout/config/config.toml").is_file());
        assert!(root.path().join("scout/config/providers.toml").is_file());
        let team =
            crate::config::paths::TeamPaths::new(crate::config::paths::team_dir(root.path()));
        let user_md = std::fs::read_to_string(team.user_md()).unwrap();
        assert!(user_md.contains("Sam"), "team USER.md: {user_md}");
        assert!(!root.path().join("scout").join("USER.md").exists());
        assert!(team.agent_role_page("scout").is_file());
        assert!(*done_rx.borrow(), "setup should be signalled complete");
        assert_eq!(
            crate::config::discover_single_agent(root.path())
                .unwrap()
                .as_deref(),
            Some("scout")
        );
    }

    #[tokio::test]
    async fn complete_setup_refuses_an_invalid_agent_name_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        for bad in ["hub", "Team", "-x", "has space", ""] {
            let (state, done_rx) = setup_state(root.path());

            let Err((status, Json(response))) =
                api_complete_setup(State(state), Json(setup_request(bad))).await
            else {
                panic!("agent name {bad:?} should be refused");
            };

            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(!response.valid);
            assert!(!*done_rx.borrow());
        }
        assert!(!root.path().join("hub/config.toml").exists());
    }

    #[tokio::test]
    async fn complete_setup_refuses_hub_only_keys_in_the_agent_config() {
        let root = tempfile::tempdir().unwrap();
        let (state, _done_rx) = setup_state(root.path());
        let mut request = setup_request("scout");
        request.config = "timezone = \"UTC\"\n".to_string();

        let result = api_complete_setup(State(state), Json(request)).await;

        assert!(result.is_err(), "timezone belongs in the hub config");
        assert!(!root.path().join("scout").exists());
    }

    #[tokio::test]
    async fn complete_setup_answers_409_when_the_agent_already_exists() {
        let root = tempfile::tempdir().unwrap();
        let agent_config = root.path().join("scout/config");
        std::fs::create_dir_all(&agent_config).unwrap();
        std::fs::write(agent_config.join("config.toml"), "# live agent\n").unwrap();
        std::fs::write(agent_config.join("providers.toml"), "# live providers\n").unwrap();
        let (state, done_rx) = setup_state(root.path());

        let Err((status, Json(response))) =
            api_complete_setup(State(state), Json(setup_request("scout"))).await
        else {
            panic!("an existing agent must not be overwritten");
        };

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!response.valid);
        assert!(response.error.unwrap().contains("already exists"));
        assert!(!*done_rx.borrow(), "setup must not be signalled complete");
        assert_eq!(
            std::fs::read_to_string(agent_config.join("providers.toml")).unwrap(),
            "# live providers\n"
        );
        assert!(!root.path().join("hub/config.toml").exists());
    }

    #[tokio::test]
    async fn complete_setup_answers_409_when_a_different_agent_exists() {
        let root = tempfile::tempdir().unwrap();
        let agent_config = root.path().join("first/config");
        std::fs::create_dir_all(&agent_config).unwrap();
        std::fs::write(agent_config.join("config.toml"), "").unwrap();
        let (state, _done_rx) = setup_state(root.path());

        let Err((status, Json(response))) =
            api_complete_setup(State(state), Json(setup_request("second"))).await
        else {
            panic!("setup creates only the first agent");
        };

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(response.error.unwrap().contains("first"));
        assert!(!root.path().join("second").exists());
    }

    #[tokio::test]
    async fn an_interrupted_setup_leaves_no_discoverable_agent() {
        let root = tempfile::tempdir().unwrap();
        // A directory where providers.toml must go makes that write fail
        // after the workspace exists but before config.toml is written.
        std::fs::create_dir_all(root.path().join("scout/config/providers.toml")).unwrap();
        let (state, done_rx) = setup_state(root.path());

        let result = api_complete_setup(State(state), Json(setup_request("scout"))).await;

        let Err((status, _)) = result else {
            panic!("the blocked providers.toml write should fail setup");
        };
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!*done_rx.borrow());
        assert!(!root.path().join("scout/config/config.toml").exists());
        assert!(
            crate::config::discover_agents(root.path())
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_failed_setup_can_be_retried() {
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join("scout/config/providers.toml");
        std::fs::create_dir_all(&blocker).unwrap();
        let (first_state, _first_done_rx) = setup_state(root.path());
        assert!(
            api_complete_setup(State(first_state), Json(setup_request("scout")))
                .await
                .is_err()
        );

        std::fs::remove_dir(&blocker).unwrap();
        let (retry_state, done_rx) = setup_state(root.path());
        let Json(response) = api_complete_setup(State(retry_state), Json(setup_request("scout")))
            .await
            .unwrap();

        assert!(response.valid);
        assert!(*done_rx.borrow());
        assert_eq!(
            crate::config::discover_agents(root.path()).unwrap(),
            vec!["scout".to_string()]
        );
    }
}
