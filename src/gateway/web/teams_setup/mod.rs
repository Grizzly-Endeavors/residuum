//! Microsoft Teams setup wizard protocol types and endpoints.
//!
//! Route handlers and background job runner integration for Microsoft 365 Agents Toolkit.

use std::path::PathBuf;

use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::gateway::web::ConfigApiState;
use crate::interfaces::teams::setup_job::{SetupJobError, get_or_create_manager};

pub mod types;

pub use types::*;

impl Default for CreatedResources {
    fn default() -> Self {
        Self {
            bot_id: None,
            teams_app_id: None,
            tenant_id: None,
            entra_url: "https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationMenuBlade/~/Overview/appId/".to_string(),
            dev_portal_url: "https://dev.teams.microsoft.com/apps/".to_string(),
        }
    }
}

/// Helper to resolve the root `~/.residuum` directory from `ConfigApiState`.
fn get_residuum_root(state: &ConfigApiState) -> PathBuf {
    state
        .hub_dir
        .parent()
        .unwrap_or(&state.hub_dir)
        .to_path_buf()
}

/// Query parameter for log filtering in `GET /api/teams-setup/job`.
#[derive(Debug, Deserialize)]
struct JobQuery {
    log_since: Option<u64>,
}

/// Router for the Microsoft Teams setup wizard API endpoints.
pub fn teams_setup_api_router() -> axum::Router<ConfigApiState> {
    axum::Router::new()
        .route("/api/teams-setup/prereqs", get(api_teams_setup_prereqs))
        .route(
            "/api/teams-setup/job",
            get(api_teams_setup_job_get)
                .post(api_teams_setup_job_post)
                .delete(api_teams_setup_job_delete),
        )
        .route(
            "/api/teams-setup/job/redirect",
            post(api_teams_setup_job_redirect),
        )
        .route(
            "/api/teams-setup/job/cancel",
            post(api_teams_setup_job_cancel),
        )
        .route(
            "/api/teams-setup/job/retry",
            post(api_teams_setup_job_retry),
        )
        .route(
            "/api/teams-setup/job/install-app",
            post(api_teams_setup_job_install_app),
        )
        .route(
            "/api/teams-setup/job/package",
            get(api_teams_setup_job_package),
        )
        .route("/api/teams-setup/cleanup", post(api_teams_setup_cleanup))
}

/// `GET /api/teams-setup/prereqs`
async fn api_teams_setup_prereqs(State(state): State<ConfigApiState>) -> impl IntoResponse {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    let prereqs = manager.get_prereqs(&state.agent_name).await;
    Json(prereqs)
}

/// `GET /api/teams-setup/job`
async fn api_teams_setup_job_get(
    State(state): State<ConfigApiState>,
    Query(query): Query<JobQuery>,
) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.get_job(&state.agent_name, query.log_since).await {
        Some(job) => (StatusCode::OK, Json(job)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "No Teams setup job found for this agent"
            })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/job`
async fn api_teams_setup_job_post(
    State(state): State<ConfigApiState>,
    Json(payload): Json<TeamsSetupStart>,
) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.start_job(&state.agent_name, payload).await {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(SetupJobError::Conflict(job)) => (StatusCode::CONFLICT, Json(*job)).into_response(),
        Err(SetupJobError::BadRequest(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/job/redirect`
async fn api_teams_setup_job_redirect(
    State(state): State<ConfigApiState>,
    Json(payload): Json<TeamsSetupRedirect>,
) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager
        .forward_redirect(&state.agent_name, &payload.url)
        .await
    {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(SetupJobError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup job found" })),
        )
            .into_response(),
        Err(SetupJobError::BadRequest(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/job/cancel`
async fn api_teams_setup_job_cancel(State(state): State<ConfigApiState>) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.cancel_job(&state.agent_name).await {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(SetupJobError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup job found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/job/retry`
async fn api_teams_setup_job_retry(State(state): State<ConfigApiState>) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.retry_job(&state.agent_name).await {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(SetupJobError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup job found" })),
        )
            .into_response(),
        Err(SetupJobError::Conflict(job)) => (StatusCode::CONFLICT, Json(*job)).into_response(),
        Err(SetupJobError::BadRequest(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/job/install-app`
async fn api_teams_setup_job_install_app(State(state): State<ConfigApiState>) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.install_app(&state.agent_name).await {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(SetupJobError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup job found" })),
        )
            .into_response(),
        Err(SetupJobError::BadRequest(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/teams-setup/job/package`
async fn api_teams_setup_job_package(State(state): State<ConfigApiState>) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    let Ok(pkg_path) = manager.package_path(&state.agent_name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup package found" })),
        )
            .into_response();
    };

    match tokio::fs::read(&pkg_path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "application/zip"),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    "attachment; filename=\"appPackage.zip\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("failed to read package file: {e}")
            })),
        )
            .into_response(),
    }
}

/// `DELETE /api/teams-setup/job`
async fn api_teams_setup_job_delete(State(state): State<ConfigApiState>) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    match manager.delete_job(&state.agent_name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(SetupJobError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No Teams setup job found" })),
        )
            .into_response(),
        Err(SetupJobError::Conflict(_)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "Cannot delete a running Teams setup job"
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/teams-setup/cleanup`
async fn api_teams_setup_cleanup(
    State(state): State<ConfigApiState>,
    Json(payload): Json<CleanupRequest>,
) -> Response {
    let root = get_residuum_root(&state);
    let manager = get_or_create_manager(&root);
    let result = manager.cleanup(&state.agent_name, payload).await;
    (StatusCode::OK, Json(result)).into_response()
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::shadow_unrelated,
    reason = "test assertions index JSON and test full flow sequentially"
)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::gateway::web::{ConfigApiState, WorkspaceScope};
    use crate::interfaces::teams::atk_runner::AtkRunnerOverrides;
    use crate::interfaces::teams::setup_job::{TeamsSetupJobManager, register_test_manager};

    fn test_state(root: &Path, agent: &str) -> ConfigApiState {
        ConfigApiState {
            hub_dir: root.join("hub"),
            config_dir: root.join(agent).join("config"),
            agent_name: agent.to_string(),
            workspace_dir: root.join(agent),
            memory_dir: None,
            reload_tx: None,
            checkpoints: crate::checkpoints::test_engine(),
            team: None,
            scope: WorkspaceScope::Agent,
        }
    }

    fn make_script(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    fn setup_mock_env(temp: &tempfile::TempDir) -> (PathBuf, AtkRunnerOverrides) {
        let root = temp.path().to_path_buf();
        let bin_dir = root.join("mock-bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let ext = if cfg!(windows) { ".cmd" } else { "" };
        let mock_node = bin_dir.join(format!("node{ext}"));
        if cfg!(windows) {
            make_script(&mock_node, "@echo off\necho v20.11.0\nexit /b 0\n");
        } else {
            make_script(&mock_node, "#!/bin/sh\necho \"v20.11.0\"\nexit 0\n");
        }

        let mock_npm = bin_dir.join(format!("npm{ext}"));
        if cfg!(windows) {
            make_script(&mock_npm, "@echo off\necho 10.2.4\nexit /b 0\n");
        } else {
            make_script(&mock_npm, "#!/bin/sh\necho \"10.2.4\"\nexit 0\n");
        }

        let mock_atk = bin_dir.join(format!("atk{ext}"));
        if cfg!(windows) {
            let ps1_path = bin_dir.join("atk.ps1");
            fs::write(
                &ps1_path,
                r#"$cmd = $args[0]
$sub = $args[1]
if ($cmd -eq "--version") { Write-Output "1.1.17"; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "list") { Write-Output "Your Microsoft 365 account is: user@example.com."; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "logout") { exit 0 }
if ($cmd -eq "install") { exit 0 }
if ($cmd -eq "provision") {
    $folder = ""
    for ($i = 0; $i -lt $args.Length; $i++) {
        if ($args[$i] -eq "--folder" -and $i + 1 -lt $args.Length) {
            $folder = $args[$i + 1]
            break
        }
    }
    if ($folder -ne "") {
        New-Item -ItemType Directory -Force -Path "$folder\env" | Out-Null
        $envRes = @"
BOT_ID=mock-bot-id-123
TEAMS_APP_TENANT_ID=mock-tenant-id-456
TEAMS_APP_ID=mock-teams-app-id-789
"@
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum", $envRes)
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum.user", "SECRET_BOT_PASSWORD=mock-password-sec`n")
        New-Item -ItemType Directory -Force -Path "$folder\appPackage\build" | Out-Null
        [System.IO.File]::WriteAllText("$folder\appPackage\build\appPackage.residuum.zip", "zip`n")
    }
    exit 0
}
exit 0
"#,
            )
            .unwrap();
            make_script(
                &mock_atk,
                "@echo off\nif \"%~1\"==\"--version\" ( echo 1.1.17 & exit /b 0 )\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dpn0.ps1\" %*\nexit /b %ERRORLEVEL%\n",
            );
        } else {
            make_script(
                &mock_atk,
                r#"#!/bin/sh
cmd="$1"
sub="$2"
if [ "$cmd" = "--version" ]; then echo "1.1.17"; exit 0; fi
if [ "$cmd" = "auth" ] && [ "$sub" = "list" ]; then echo "Your Microsoft 365 account is: user@example.com."; exit 0; fi
if [ "$cmd" = "auth" ] && [ "$sub" = "logout" ]; then exit 0; fi
if [ "$cmd" = "provision" ]; then
    folder=""
    while [ "$#" -gt 0 ]; do
        if [ "$1" = "--folder" ]; then folder="$2"; shift 2; else shift; fi
    done
    mkdir -p "$folder/env"
    cat << 'EOF' > "$folder/env/.env.residuum"
BOT_ID=mock-bot-id-123
TEAMS_APP_TENANT_ID=mock-tenant-id-456
TEAMS_APP_ID=mock-teams-app-id-789
EOF
    cat << 'EOF' > "$folder/env/.env.residuum.user"
SECRET_BOT_PASSWORD=mock-password-sec
EOF
    mkdir -p "$folder/appPackage/build"
    echo "zip" > "$folder/appPackage/build/appPackage.residuum.zip"
    exit 0
fi
if [ "$cmd" = "install" ]; then exit 0; fi
exit 0
"#,
            );
        }

        let overrides = AtkRunnerOverrides {
            node_bin: Some(mock_node),
            npm_bin: Some(mock_npm),
            atk_bin: Some(mock_atk),
        };

        (root, overrides)
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn test_prereqs_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        register_test_manager(&root, mgr);

        let state = test_state(&root, "test-agent");
        let app = teams_setup_api_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams-setup/prereqs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let val = body_json(resp).await;
        assert_eq!(val["node"]["found"], true);
        assert_eq!(val["npm"]["found"], true);
        assert_eq!(val["atk"]["installed"], true);
        assert_eq!(
            val["manual_guide_url"],
            "https://residuum.dev/docs/guides/teams-setup"
        );
    }

    #[tokio::test]
    async fn test_job_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        register_test_manager(&root, mgr);

        let state = test_state(&root, "test-agent-missing");
        let app = teams_setup_api_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/teams-setup/job")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let val = body_json(resp).await;
        assert!(
            val["error"]
                .as_str()
                .unwrap()
                .contains("No Teams setup job found")
        );
    }

    #[tokio::test]
    async fn test_job_start_validation_and_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        register_test_manager(&root, mgr);

        let state = test_state(&root, "agent-flow");
        let app = teams_setup_api_router().with_state(state.clone());

        // 1. Validation failure: empty bot name
        let bad_payload = serde_json::json!({
            "form": {
                "bot_name": "",
                "short_description": "Short",
                "long_description": "Long",
                "developer_name": "Dev",
                "developer_url": "https://example.com",
                "privacy_url": null,
                "terms_url": null,
                "messaging_endpoint": "https://example.com/api/teams/messages",
                "color_icon_png_base64": null,
                "outline_icon_png_base64": null
            },
            "consent_install_cli": true,
            "replace_existing": false
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/teams-setup/job")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&bad_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let err = body_json(resp).await;
        assert!(
            err["error"]
                .as_str()
                .unwrap()
                .contains("Bot name is required.")
        );

        // 2. Start valid job
        let valid_payload = serde_json::json!({
            "form": {
                "bot_name": "My Flow Bot",
                "short_description": "Short description",
                "long_description": "Long description",
                "developer_name": "Dev",
                "developer_url": "https://example.com",
                "privacy_url": null,
                "terms_url": null,
                "messaging_endpoint": "https://example.com/api/teams/messages",
                "color_icon_png_base64": null,
                "outline_icon_png_base64": null
            },
            "consent_install_cli": true,
            "replace_existing": false
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/teams-setup/job")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&valid_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let job = body_json(resp).await;
        assert_eq!(job["agent"], "agent-flow");

        // 3. Duplicate start returns 409 Conflict with running job
        let resp_dup = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/teams-setup/job")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&valid_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_dup.status(), StatusCode::CONFLICT);
        let dup_job = body_json(resp_dup).await;
        assert_eq!(dup_job["agent"], "agent-flow");

        // 4. Poll job with log_since
        let resp_poll = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams-setup/job?log_since=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_poll.status(), StatusCode::OK);
        let poll_job = body_json(resp_poll).await;
        assert_eq!(poll_job["agent"], "agent-flow");

        // 5. Cancel job
        let resp_cancel = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/teams-setup/job/cancel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_cancel.status(), StatusCode::OK);
        let cancelled_job = body_json(resp_cancel).await;
        assert_eq!(cancelled_job["state"], "cancelled");

        // 6. Delete job
        let resp_del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/teams-setup/job")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_del.status(), StatusCode::NO_CONTENT);

        // 7. Verify 404 after delete
        let resp_after_del = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams-setup/job")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_after_del.status(), StatusCode::NOT_FOUND);

        // 8. Cleanup endpoint
        let cleanup_payload = serde_json::json!({
            "project_files": true,
            "cli": false,
            "sign_out": false
        });
        let resp_clean = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/teams-setup/cleanup")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&cleanup_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_clean.status(), StatusCode::OK);
        let clean_res = body_json(resp_clean).await;
        assert!(
            clean_res["removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "project_files")
        );

        // 9. Package download when package zip exists
        let pkg_file = root.join("agent-flow/teams-app/appPackage/build/appPackage.residuum.zip");
        fs::create_dir_all(pkg_file.parent().unwrap()).unwrap();
        fs::write(&pkg_file, b"PK\x03\x04mock-zip").unwrap();

        let resp_pkg = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/teams-setup/job/package")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_pkg.status(), StatusCode::OK);
        assert_eq!(
            resp_pkg.headers().get("Content-Type").unwrap(),
            "application/zip"
        );
    }
}
