//! Web UI static asset serving and config API endpoints.
//!
//! Embeds the `web/dist/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::Uri;
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use tokio::sync::watch;

use super::ReloadSignal;

pub(crate) mod a2a;
mod agent_keys;
pub(crate) mod artifact_identity;
pub mod checkpoints;
pub mod cloud;
pub mod config;
pub mod inbox;
pub(crate) mod memory;
pub(crate) mod model;
pub mod providers;
pub(crate) mod scheduled;
pub mod secrets;
pub(crate) mod sessions;
pub mod tracing_api;
pub mod update;
pub(crate) mod workbench;
pub mod workspace;
pub(crate) mod workspace_bulk;

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
    /// Workspace and config checkpoint repositories.
    pub checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
}

impl ConfigApiState {
    /// Checkpoint the config repository (root `config.toml`/`providers.toml`
    /// and the encrypted key stores) before a write to one of them. Never
    /// fails or blocks the write — see `crate::checkpoints`.
    pub(super) async fn checkpoint_config_before_write(&self, summary: impl Into<String>) {
        self.checkpoints
            .checkpoint_config_before_write(crate::checkpoints::CheckpointContext::system(
                crate::checkpoints::CheckpointTrigger::PreConfigWrite,
                summary,
            ))
            .await;
    }
}

/// Build the config API router.
pub(super) fn config_api_router(state: ConfigApiState) -> axum::Router {
    // Scoped to just this route via `route_layer` (which wraps every route
    // already registered on *this* router value): axum's default 2 MiB body
    // limit stays in place for every other endpoint, while text writes get
    // the 8 MiB the workspace file API promises.
    let workspace_file_router = axum::Router::new()
        .route(
            "/api/workspace/file",
            get(workspace::api_workspace_file_read)
                .put(workspace::api_workspace_file_write)
                .delete(workspace::api_workspace_delete),
        )
        .route(
            "/api/workspace/raw",
            get(workspace::api_workspace_raw_read).put(workspace::api_workspace_raw_write),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(
            workspace::TEXT_FILE_LIMIT_BYTES,
        ));

    axum::Router::new()
        .route("/api/status", get(config::api_status))
        .route("/api/config/raw", get(config::api_config_raw_get))
        .route("/api/config/raw", put(config::api_config_raw_put))
        .route("/api/config/patch", patch(config::api_config_patch))
        .route("/api/config/validate", post(config::api_config_validate))
        .route(
            "/api/config/complete-setup",
            post(config::api_complete_setup),
        )
        .route("/api/system/timezone", get(config::api_system_timezone))
        .route("/api/mcp-catalog", get(config::api_mcp_catalog))
        .route("/api/chat/history", get(config::api_chat_history))
        .route("/api/usage", get(config::api_usage))
        .route(
            "/api/providers/models",
            post(providers::api_provider_models),
        )
        .route("/api/providers/raw", get(providers::api_providers_raw_get))
        .route("/api/providers/raw", put(providers::api_providers_raw_put))
        .route(
            "/api/providers/patch",
            patch(providers::api_providers_patch),
        )
        .route(
            "/api/providers/validate",
            post(providers::api_providers_validate),
        )
        .route("/api/mcp/raw", get(config::api_mcp_raw_get))
        .route("/api/mcp/raw", put(config::api_mcp_raw_put))
        .route("/api/mcp/patch", patch(config::api_mcp_patch))
        .route("/api/agent-keys", get(agent_keys::api_agent_keys_list))
        .route("/api/agent-keys", post(agent_keys::api_agent_keys_set))
        .route(
            "/api/agent-keys/{name}",
            delete(agent_keys::api_agent_keys_delete),
        )
        .route("/api/a2a/keys", get(a2a::api_a2a_keys_list))
        .route("/api/a2a/keys", post(a2a::api_a2a_keys_create))
        .route("/api/a2a/keys/{name}", delete(a2a::api_a2a_keys_revoke))
        .route("/api/a2a/agents/raw", get(a2a::api_a2a_agents_raw_get))
        .route("/api/a2a/agents/raw", put(a2a::api_a2a_agents_raw_put))
        .route("/api/secrets", post(secrets::api_secrets_set))
        .route("/api/secrets", get(secrets::api_secrets_list))
        .route("/api/secrets/{name}", delete(secrets::api_secrets_delete))
        .route("/api/workspace/files", get(workspace::api_workspace_files))
        .route("/api/workspace/dir", post(workspace::api_workspace_mkdir))
        .route("/api/workspace/move", post(workspace::api_workspace_move))
        .route(
            "/api/workspace/validate",
            post(workspace::api_workspace_validate),
        )
        .merge(workspace_file_router)
        .route(
            "/api/workspace/tree",
            get(workspace_bulk::api_workspace_tree),
        )
        .route(
            "/api/workspace/read",
            post(workspace_bulk::api_workspace_read),
        )
        .route("/api/inbox", get(inbox::api_inbox_list))
        .route("/api/inbox/{id}/read", put(inbox::api_inbox_read))
        .route("/api/inbox/{id}/archive", post(inbox::api_inbox_archive))
        .route(
            "/api/inbox/{id}/attachments/{index}",
            get(inbox::api_inbox_attachment),
        )
        .with_state(state)
}

/// Fallback handler for serving embedded static files.
///
/// Serves the file at the requested URI path, falling back to `index.html`
/// for the web UI's client-side routes (paths without file extensions).
/// Unknown API and WebSocket paths get a 404 rather than the app shell, so a
/// client calling a missing endpoint sees the failure instead of HTML.
pub(super) async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first
    if let Some(resp) = serve_embedded(path) {
        return resp;
    }

    if is_client_route(path)
        && let Some(resp) = serve_embedded("index.html")
    {
        return resp;
    }

    Response::builder()
        .status(axum::http::StatusCode::NOT_FOUND)
        .body(axum::body::Body::from("not found"))
        .unwrap_or_default()
}

/// Whether `path` (without its leading slash) is a web UI route that the
/// client-side router handles, rather than a missing asset or server endpoint.
fn is_client_route(path: &str) -> bool {
    let first_segment = path.split('/').next().unwrap_or_default();
    !path.contains('.') && first_segment != "api" && first_segment != "ws"
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
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use axum::http::{StatusCode, header};

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
    fn serve_embedded_returns_manifest_content_type() {
        let resp = serve_embedded("manifest.webmanifest").unwrap();
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap();
        assert_eq!(
            ct.to_str().unwrap(),
            "application/manifest+json",
            "web app manifest should be served with the manifest content type"
        );
    }

    #[tokio::test]
    async fn static_handler_serves_app_shell_for_client_routes() {
        for path in [
            "/",
            "/sessions/run-1790000000000-0a1b2c3d",
            "/settings/memory",
        ] {
            let resp = static_handler(Uri::from_static(path)).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path} should be served");
            let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
            assert!(
                ct.to_str().unwrap().contains("html"),
                "{path} should get index.html"
            );
        }
    }

    #[tokio::test]
    async fn static_handler_404s_for_missing_endpoints_and_assets() {
        for path in [
            "/api/nope",
            "/api",
            "/ws/extra",
            "/assets/missing-abc123.js",
        ] {
            let resp = static_handler(Uri::from_static(path)).await;
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{path} should not get the app shell"
            );
        }
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
        use axum::extract::{Query, State};

        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };
        let Json(segment) = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();
        match segment {
            config::ChatHistorySegment::Recent {
                messages,
                next_cursor,
            } => {
                assert!(messages.is_empty(), "setup mode should have no messages");
                assert!(next_cursor.is_none(), "setup mode should have no cursor");
            }
            config::ChatHistorySegment::Episode { .. } => {
                panic!("expected Recent segment in setup mode");
            }
        }
    }

    #[tokio::test]
    async fn api_usage_returns_zero_default_when_no_memory_dir() {
        use axum::Json;
        use axum::extract::State;

        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };
        let Json(totals) = config::api_usage(State(state)).await;
        assert_eq!(totals, crate::agent::usage::SessionUsageTotals::default());
    }

    #[tokio::test]
    async fn api_usage_reads_the_persisted_totals_file() {
        use axum::Json;
        use axum::extract::State;

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();
        let mut totals = crate::agent::usage::SessionUsageTotals::default();
        totals.accumulate(Some(crate::inference::Usage {
            input_tokens: 300,
            output_tokens: 60,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));
        crate::agent::usage::save_session_usage_totals(
            &memory_dir.join("usage_totals.json"),
            &totals,
        )
        .await;

        let state = ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().to_path_buf(),
            memory_dir: Some(memory_dir),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };
        let Json(loaded) = config::api_usage(State(state)).await;
        assert_eq!(loaded, totals);
    }

    #[tokio::test]
    async fn chat_history_returns_empty_when_file_missing() {
        use axum::Json;
        use axum::extract::{Query, State};

        let state = ConfigApiState {
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            memory_dir: Some(PathBuf::from("/tmp/residuum-test-nonexistent-memory")),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };
        let Json(segment) = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();
        match segment {
            config::ChatHistorySegment::Recent {
                messages,
                next_cursor,
            } => {
                assert!(messages.is_empty(), "missing file should have no messages");
                assert!(next_cursor.is_none(), "no episodes yet");
            }
            config::ChatHistorySegment::Episode { .. } => {
                panic!("expected Recent segment when recent_messages.json is missing");
            }
        }
    }

    #[tokio::test]
    async fn chat_history_recent_exposes_latest_episode_cursor() {
        use crate::memory::episode_store::{episode_jsonl_path, write_episode_transcript};
        use crate::memory::types::Episode;
        use axum::Json;
        use axum::extract::{Query, State};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        // Write two episodes on disk so the cursor should point at ep-002.
        for (id, date) in [
            (
                "ep-001",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            ),
            (
                "ep-002",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
            ),
        ] {
            let episode = Episode {
                id: id.to_string(),
                date,
                observations: vec![],
            };
            write_episode_transcript(
                &episodes_dir,
                &episode,
                &[crate::inference::Message::user("hi")],
            )
            .await
            .unwrap();
            // Sanity check the file lands where we expect.
            assert!(episode_jsonl_path(&episodes_dir, &episode).exists());
        }

        let state = ConfigApiState {
            config_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            memory_dir: Some(memory_dir),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };
        let Json(segment) = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();

        match segment {
            config::ChatHistorySegment::Recent { next_cursor, .. } => {
                assert_eq!(next_cursor.as_deref(), Some("ep-002"));
            }
            config::ChatHistorySegment::Episode { .. } => panic!("expected Recent"),
        }
    }

    #[tokio::test]
    async fn chat_history_fetches_specific_episode_with_prev_cursor() {
        use crate::memory::episode_store::write_episode_transcript;
        use crate::memory::types::Episode;
        use axum::Json;
        use axum::extract::{Query, State};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        for (id, date, body) in [
            (
                "ep-001",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
                "oldest",
            ),
            (
                "ep-002",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
                "middle",
            ),
            (
                "ep-003",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap(),
                "newest",
            ),
        ] {
            let episode = Episode {
                id: id.to_string(),
                date,
                observations: vec![],
            };
            write_episode_transcript(
                &episodes_dir,
                &episode,
                &[crate::inference::Message::user(body)],
            )
            .await
            .unwrap();
        }

        let state = ConfigApiState {
            config_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            memory_dir: Some(memory_dir),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };

        let Json(segment) = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery {
                episode: Some("ep-002".to_string()),
            }),
        )
        .await
        .unwrap();

        match segment {
            config::ChatHistorySegment::Episode {
                episode_id,
                messages,
                next_cursor,
                ..
            } => {
                assert_eq!(episode_id, "ep-002");
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].message.content, "middle");
                assert_eq!(
                    next_cursor.as_deref(),
                    Some("ep-001"),
                    "cursor should walk backward"
                );
            }
            config::ChatHistorySegment::Recent { .. } => panic!("expected Episode"),
        }
    }

    #[tokio::test]
    async fn chat_history_missing_episode_returns_404() {
        use axum::extract::{Query, State};
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        let state = ConfigApiState {
            config_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            memory_dir: Some(memory_dir),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };

        let err = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery {
                episode: Some("ep-999".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn chat_history_propagates_parse_errors_instead_of_silently_returning_empty() {
        // Regression test: a malformed recent_messages.json used to be swallowed
        // at debug! level and surfaced as an empty Recent segment, hiding real
        // file corruption and making the chat feed look empty when it wasn't.
        // The handler must now return a 5xx so the frontend can surface the
        // failure to the user instead of silently dropping the history.
        use axum::extract::{Query, State};
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        // Timestamp uses ISO seconds-precision, which minute_format rejects.
        // A single bad row here fails the whole file parse.
        let malformed = r#"[{"role":"user","content":"hi","timestamp":"2026-04-12T15:00:30","visibility":"user"}]"#;
        tokio::fs::write(memory_dir.join("recent_messages.json"), malformed)
            .await
            .unwrap();

        let state = ConfigApiState {
            config_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            memory_dir: Some(memory_dir),
            reload_tx: None,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };

        let err = config::api_chat_history(
            State(state),
            Query(config::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            StatusCode::INTERNAL_SERVER_ERROR,
            "parse failures must not be silently converted to empty history",
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
            checkpoints: crate::checkpoints::test_engine(),
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
