//! The hub routes that concern the whole process: cloud, update, shutdown,
//! tracing, and the hub and team checkpoint repositories.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use super::state::HubHttpState;
use crate::gateway::remote_control_guard::reject_remote_shutdown_and_disconnect;
use crate::gateway::web;

/// Cloud connection routes. Disconnect is refused over the tunnel, since a
/// remote disconnect leaves no way to reconnect. The relay's OAuth callback
/// stays at the root because the relay redirects the browser to it.
pub(super) fn cloud_routes(hub: &HubHttpState) -> Router {
    let state = web::cloud::CloudApiState {
        hub_dir: hub.hub_dir.clone(),
        reload_tx: hub.reload_tx.clone(),
        tunnel_status_rx: hub.tunnel_status_rx.clone(),
        secret_lock: Arc::clone(&hub.secret_lock),
    };
    Router::new()
        .route("/api/hub/cloud/status", get(web::cloud::api_cloud_status))
        .route("/cloud/callback", get(web::cloud::cloud_callback))
        .with_state(state.clone())
        .merge(
            Router::new()
                .route(
                    "/api/hub/cloud/disconnect",
                    post(web::cloud::api_cloud_disconnect),
                )
                .route_layer(axum::middleware::from_fn(
                    reject_remote_shutdown_and_disconnect,
                ))
                .with_state(state),
        )
}

/// Update and shutdown routes. Shutdown is refused over the tunnel, since a
/// remote shutdown leaves no way to bring the hub back.
pub(super) fn update_routes(hub: &HubHttpState) -> Router {
    let state = web::update::UpdateApiState {
        update_status: Arc::clone(&hub.update_status),
        restart_tx: hub.restart_tx.clone(),
        gateway_shutdown_tx: hub.shutdown_tx.clone(),
        hub_dir: hub.hub_dir.clone(),
    };
    Router::new()
        .route(
            "/api/hub/update/status",
            get(web::update::api_update_status),
        )
        .route("/api/hub/update/check", post(web::update::api_update_check))
        .route("/api/hub/update/apply", post(web::update::api_update_apply))
        .route(
            "/api/hub/update/restart",
            post(web::update::api_update_restart),
        )
        .with_state(state.clone())
        .merge(
            Router::new()
                .route("/api/hub/shutdown", post(web::update::api_shutdown))
                .route_layer(axum::middleware::from_fn(
                    reject_remote_shutdown_and_disconnect,
                ))
                .with_state(state),
        )
}

/// The tracing and observability routes.
pub(super) fn tracing_routes(hub: &HubHttpState) -> Router {
    use web::tracing_api as api;

    let state = api::TracingApiState {
        service: Arc::clone(&hub.tracing_service),
        client_context: Arc::clone(&hub.client_context),
        active_subagents: Arc::clone(&hub.active_subagents),
    };
    Router::new()
        .route("/api/hub/tracing/status", get(api::api_tracing_status))
        .route(
            "/api/hub/tracing/error-reporting",
            post(api::api_tracing_error_reporting),
        )
        .route("/api/hub/tracing/sanitize", post(api::api_tracing_sanitize))
        .route(
            "/api/hub/tracing/otel/endpoints",
            get(api::api_tracing_otel_list)
                .post(api::api_tracing_otel_add)
                .delete(api::api_tracing_otel_remove),
        )
        .route(
            "/api/hub/tracing/otel/test",
            post(api::api_tracing_otel_test),
        )
        .route("/api/hub/tracing/dump", post(api::api_tracing_dump))
        .route(
            "/api/hub/tracing/stream/start",
            post(api::api_tracing_stream_start),
        )
        .route(
            "/api/hub/tracing/stream/stop",
            post(api::api_tracing_stream_stop),
        )
        .route(
            "/api/hub/tracing/bug-report",
            post(api::api_tracing_bug_report),
        )
        .route("/api/hub/tracing/feedback", post(api::api_tracing_feedback))
        .with_state(state)
}

/// Checkpoint history and restore for the hub config and team repositories.
pub(super) fn checkpoint_routes(hub: &HubHttpState) -> Router {
    web::checkpoints::checkpoints_api_router(
        web::checkpoints::CheckpointApiState {
            checkpoints: Arc::clone(&hub.checkpoints),
            repos: web::checkpoints::HUB_REPOS,
        },
        "/api/hub/checkpoints",
    )
}

/// The team's workspace file API and its workbench routes.
pub(super) fn team_routes(hub: &HubHttpState) -> Router {
    let team_root = hub.team.root().to_path_buf();
    let team_view = hub.team.view_for_user(&team_root);
    let files = web::ConfigApiState {
        hub_dir: hub.hub_dir.clone(),
        config_dir: hub.hub_dir.clone(),
        agent_name: crate::workspace::team_files::TEAM_PREFIX.to_string(),
        workspace_dir: team_root.clone(),
        memory_dir: None,
        reload_tx: Some(hub.reload_tx.clone()),
        checkpoints: Arc::clone(&hub.checkpoints),
        team: Some(team_view.clone()),
        scope: web::WorkspaceScope::Team,
    };
    let workbench = web::workbench::workbench_api_router(web::workbench::WorkbenchApiState {
        dir: crate::config::paths::TeamPaths::new(team_root).workbench_dir(),
        serving: hub.workbench_serving.clone(),
        tunnel_status_rx: hub.tunnel_status_rx.clone(),
        checkpoints: Arc::clone(&hub.checkpoints),
        team: Some(team_view),
    });
    web::team_workspace_api_router(files).merge(workbench)
}

/// The hub-level config, secret, key, and onboarding routes.
pub(super) fn hub_config_routes(hub: &HubHttpState) -> Router {
    web::hub_api_router(web::HubApiState {
        hub_dir: hub.hub_dir.clone(),
        reload_tx: hub.reload_tx.clone(),
        setup_done: hub.setup_done.clone(),
        secret_lock: Arc::clone(&hub.secret_lock),
        checkpoints: Arc::clone(&hub.checkpoints),
        team: hub.team.clone(),
    })
}
