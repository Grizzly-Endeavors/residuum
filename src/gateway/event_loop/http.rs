//! HTTP server setup and adapter spawning in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::Config;
use crate::gateway::types::{GatewayState, ReloadSignal, ServerCommand, StopRequest};
use crate::util::FatalError;

use crate::gateway::web;
use crate::gateway::ws::ws_handler;

/// Bundled senders for spawning a chat adapter (Discord or Telegram).
#[derive(Clone)]
pub struct AdapterSenders {
    pub publisher: crate::bus::Publisher,
    pub bus_handle: crate::bus::BusHandle,
    pub reload: tokio::sync::watch::Sender<ReloadSignal>,
    pub command: mpsc::Sender<ServerCommand>,
    pub stop: mpsc::Sender<StopRequest>,
    /// Where the adapter registers the conversations it can reach.
    pub(crate) conversations: crate::interfaces::conversations::ConversationDirectory,
}

/// Lifecycle handles returned from spawning chat adapters.
pub struct AdapterHandles {
    pub discord_handle: Option<tokio::task::JoinHandle<()>>,
    pub discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub telegram_handle: Option<tokio::task::JoinHandle<()>>,
    pub telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub teams_handle: Option<tokio::task::JoinHandle<()>>,
    pub teams_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub a2a_handle: Option<tokio::task::JoinHandle<()>>,
    pub a2a_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// The live agent card, so a workspace-file reload can update it without
    /// restarting the listener. `None` when A2A is disabled.
    pub a2a_card_state: Option<crate::a2a::SharedCardState>,
}

/// State bundle for the smaller, cross-cutting API routers, grouped so
/// [`build_gateway_app`] doesn't grow another positional parameter for each
/// one (and trip `clippy::too_many_arguments`).
pub struct ExtraApiStates {
    pub memory: web::memory::MemoryApiState,
    pub model: web::model::ModelApiState,
    pub a2a_agents: web::a2a::A2aAgentsStatusState,
}

/// Build the gateway app with WebSocket, webhook, cloud, update, and config API routes.
pub fn build_gateway_app(
    state: GatewayState,
    config_api_state: web::ConfigApiState,
    update_api_state: web::update::UpdateApiState,
    tracing_api_state: web::tracing_api::TracingApiState,
    workbench_serving: crate::workbench::server::WorkbenchServing,
    extra: ExtraApiStates,
) -> axum::Router {
    use axum::routing::{get, post};

    // Always mounted: the table is swapped on reload, so webhooks added later
    // work without rebinding the server. Unknown names get a 404.
    let webhook_router = axum::Router::new()
        .route(
            "/webhook/{name}",
            axum::routing::post(crate::interfaces::webhook::webhook_handler),
        )
        .with_state(crate::interfaces::webhook::WebhookState {
            publisher: state.publisher.clone(),
            webhooks: state.webhooks.clone(),
            tz: state.tz,
        });

    let cloud_router = {
        let cloud_state = web::cloud::CloudApiState {
            config_dir: config_api_state.config_dir.clone(),
            reload_tx: state.reload_tx.clone(),
            tunnel_status_rx: state.tunnel_status_rx.clone(),
            secret_lock: Arc::clone(&config_api_state.secret_lock),
        };
        axum::Router::new()
            .route("/api/cloud/status", get(web::cloud::api_cloud_status))
            .route("/cloud/callback", get(web::cloud::cloud_callback))
            .route(
                "/api/cloud/disconnect",
                post(web::cloud::api_cloud_disconnect),
            )
            .with_state(cloud_state)
    };

    let update_router = axum::Router::new()
        .route("/api/update/status", get(web::update::api_update_status))
        .route("/api/update/check", post(web::update::api_update_check))
        .route("/api/update/apply", post(web::update::api_update_apply))
        .route("/api/update/restart", post(web::update::api_update_restart))
        .route("/api/shutdown", post(web::update::api_shutdown))
        .with_state(update_api_state);

    let tracing_router = tracing_api_router(tracing_api_state);

    let file_router = axum::Router::new()
        .route(
            "/api/files/{id}",
            get(crate::gateway::file_server::serve_file),
        )
        .with_state(state.file_registry.clone());

    let sessions_router = web::sessions::sessions_api_router(web::sessions::SessionsApiState {
        registry: Arc::clone(&state.session_registry),
        store: Arc::clone(&state.session_store),
        tz: state.tz,
        messenger: Arc::clone(&state.agent_messenger),
        publisher: state.publisher.clone(),
        skill_state: Arc::clone(&state.skill_state),
    });

    let workbench_router =
        web::workbench::workbench_api_router(web::workbench::WorkbenchApiState {
            dir: crate::workspace::layout::WorkspaceLayout::new(&config_api_state.workspace_dir)
                .workbench_dir(),
            serving: workbench_serving,
            tunnel_status_rx: state.tunnel_status_rx.clone(),
        });

    let agent_inbox_router = web::inbox::agent_inbox_api_router(state.clone());
    let memory_router = web::memory::memory_api_router(extra.memory);
    let model_router = web::model::model_api_router(extra.model);
    let a2a_agents_router = web::a2a::a2a_agents_status_router(extra.a2a_agents);

    axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .merge(webhook_router)
        .merge(file_router)
        .merge(sessions_router)
        .merge(cloud_router)
        .merge(update_router)
        .merge(tracing_router)
        .merge(workbench_router)
        .merge(agent_inbox_router)
        .merge(memory_router)
        .merge(model_router)
        .merge(a2a_agents_router)
        .merge(web::config_api_router(config_api_state))
        .fallback(web::static_handler)
        .layer(axum::middleware::from_fn(
            crate::gateway::cross_site::reject_cross_site_requests,
        ))
}

/// Build the tracing API router with all observability endpoints.
fn tracing_api_router(state: web::tracing_api::TracingApiState) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/api/tracing/status",
            get(web::tracing_api::api_tracing_status),
        )
        .route(
            "/api/tracing/error-reporting",
            post(web::tracing_api::api_tracing_error_reporting),
        )
        .route(
            "/api/tracing/sanitize",
            post(web::tracing_api::api_tracing_sanitize),
        )
        .route(
            "/api/tracing/otel/endpoints",
            get(web::tracing_api::api_tracing_otel_list)
                .post(web::tracing_api::api_tracing_otel_add)
                .delete(web::tracing_api::api_tracing_otel_remove),
        )
        .route(
            "/api/tracing/otel/test",
            post(web::tracing_api::api_tracing_otel_test),
        )
        .route(
            "/api/tracing/dump",
            post(web::tracing_api::api_tracing_dump),
        )
        .route(
            "/api/tracing/stream/start",
            post(web::tracing_api::api_tracing_stream_start),
        )
        .route(
            "/api/tracing/stream/stop",
            post(web::tracing_api::api_tracing_stream_stop),
        )
        .route(
            "/api/tracing/bug-report",
            post(web::tracing_api::api_tracing_bug_report),
        )
        .route(
            "/api/tracing/feedback",
            post(web::tracing_api::api_tracing_feedback),
        )
        .with_state(state)
}

/// Spawn an axum server on a pre-bound listener with graceful shutdown.
pub(crate) fn spawn_server_with_listener(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    http_shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    let mut shutdown_rx = http_shutdown_tx.subscribe();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|v| *v).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    })
}

/// Bind the HTTP server and spawn it as a background task.
///
/// # Errors
/// Returns `FatalError` if the listener cannot bind to the configured address.
pub async fn spawn_http_server(
    cfg: &Config,
    app: axum::Router,
    http_shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> Result<tokio::task::JoinHandle<()>, FatalError> {
    let addr = cfg.gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");
    if cfg.gateway.bind != "127.0.0.1" && cfg.gateway.bind != "localhost" {
        tracing::warn!(
            bind = %cfg.gateway.bind,
            "web UI is exposed on a non-loopback address with no authentication"
        );
    }
    Ok(spawn_server_with_listener(listener, app, http_shutdown_tx))
}

/// Spawn the Discord, Telegram, and Teams adapters that are configured.
pub fn spawn_adapters(cfg: &Config, senders: &AdapterSenders, tz: chrono_tz::Tz) -> AdapterHandles {
    let (mut discord_handle, mut discord_shutdown_tx) = (None, None);
    if let Some(ref discord_cfg) = cfg.discord {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::discord::DiscordInterface::new(
            discord_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        discord_handle = Some(crate::util::spawn_monitored("discord", async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "discord interface failed");
            }
        }));
        discord_shutdown_tx = Some(tx);
        tracing::info!("discord interface started");
    }

    let (mut telegram_handle, mut telegram_shutdown_tx) = (None, None);
    if let Some(ref telegram_cfg) = cfg.telegram {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::telegram::TelegramInterface::new(
            telegram_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        telegram_handle = Some(crate::util::spawn_monitored("telegram", async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "telegram interface failed");
            }
        }));
        telegram_shutdown_tx = Some(tx);
        tracing::info!("telegram interface started");
    }

    let (mut teams_handle, mut teams_shutdown_tx) = (None, None);
    if let Some(ref teams_cfg) = cfg.teams {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::teams::TeamsInterface::new(
            teams_cfg.clone(),
            senders.clone(),
            cfg.gateway.bind.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        teams_handle = Some(crate::util::spawn_monitored("teams", async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "teams interface failed");
            }
        }));
        teams_shutdown_tx = Some(tx);
    }

    let (mut a2a_handle, mut a2a_shutdown_tx, mut a2a_card_state) = (None, None, None);
    if cfg.a2a.enabled {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let (handle, card_state) = build_a2a_listener(cfg, rx);
        a2a_handle = Some(handle);
        a2a_shutdown_tx = Some(tx);
        a2a_card_state = Some(card_state);
        tracing::info!(visibility = %cfg.a2a.visibility, "a2a interface started");
    }

    AdapterHandles {
        discord_handle,
        discord_shutdown_tx,
        telegram_handle,
        telegram_shutdown_tx,
        teams_handle,
        teams_shutdown_tx,
        a2a_handle,
        a2a_shutdown_tx,
        a2a_card_state,
    }
}

/// Build the live agent card and spawn the A2A listener task. Shared between
/// initial startup and reload: both rebuild the listener from scratch when
/// `[a2a]` changes, since a visibility flip and a new base URL both need a
/// fresh card as well as a fresh listener.
pub(crate) fn build_a2a_listener(
    cfg: &Config,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> (tokio::task::JoinHandle<()>, crate::a2a::SharedCardState) {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&cfg.workspace_dir);
    let card_runtime = crate::a2a::CardRuntime::from_config(&cfg.a2a, &cfg.gateway.bind);
    let card_state =
        crate::a2a::CardState::load_or_default(&layout.agent_card_json(), &card_runtime);
    let keys = crate::a2a::A2aKeys::new_shared(&cfg.config_dir);
    let listener = crate::a2a::A2aListener::new(
        cfg.a2a.clone(),
        cfg.gateway.bind.clone(),
        Arc::new(crate::a2a::StubHandler),
        Arc::clone(&card_state),
        keys,
        Arc::new(|| Some(Arc::<str>::from(crate::tunnel::tunnel_nonce()))),
        shutdown_rx,
    );
    let handle = crate::util::spawn_monitored("a2a", async move {
        if let Err(e) = listener.start().await {
            tracing::error!(error = %e, "a2a interface failed");
        }
    });
    (handle, card_state)
}
