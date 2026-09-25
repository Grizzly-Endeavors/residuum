//! HTTP server setup and adapter spawning in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::bus::BusHandle;
use crate::config::Config;
use crate::gateway::types::{GatewayState, ReloadSignal, ServerCommand, StopRequest};
use crate::skills::SharedSkillState;
use crate::tunnel::TunnelStatus;
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
    /// Looked up when a `/stop` command targets a conversation session
    /// rather than main — see [`crate::interfaces::dispatch_stop_request`].
    pub session_registry: Arc<SessionRegistry>,
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
    /// This instance's current A2A public URL, for the web settings API to
    /// read later. `None` when A2A is disabled.
    pub a2a_public_url: Option<crate::a2a::SharedA2aPublicUrl>,
}

/// What [`build_a2a_listener`] needs beyond `Config`: live runtime state
/// (sessions, messaging, skills, the bus, and the tunnel's status) rather
/// than anything derivable from config alone.
pub(crate) struct A2aListenerDeps {
    pub session_registry: Arc<SessionRegistry>,
    pub agent_messenger: Arc<AgentMessenger>,
    pub skill_state: SharedSkillState,
    pub bus_handle: BusHandle,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    /// Turns `true` once the session spawn listener is subscribed. Restart
    /// continuations wait on it: a session spawn published before then has
    /// no listener and would be lost.
    pub sessions_ready: tokio::sync::watch::Receiver<bool>,
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

    let a2a_tunnel_status_rx = state.tunnel_status_rx.clone();

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
        .route(
            "/api/files/workspace",
            get(crate::gateway::file_server::serve_workspace_file),
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
        .merge(web::a2a::a2a_status_router(web::a2a::A2aStatusApiState {
            config: config_api_state.clone(),
            tunnel_status_rx: a2a_tunnel_status_rx,
        }))
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

/// Spawn the Discord, Telegram, Teams, and A2A adapters that are configured.
pub async fn spawn_adapters(
    cfg: &Config,
    senders: &AdapterSenders,
    tz: chrono_tz::Tz,
    a2a_deps: A2aListenerDeps,
) -> AdapterHandles {
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

    let (mut a2a_handle, mut a2a_shutdown_tx, mut a2a_card_state, mut a2a_public_url) =
        (None, None, None, None);
    if cfg.a2a.enabled {
        let (tx, rx) = tokio::sync::watch::channel(false);
        match build_a2a_listener(cfg, a2a_deps, rx).await {
            Ok((handle, card_state, public_url)) => {
                tracing::info!(
                    visibility = %cfg.a2a.visibility,
                    public_url = %public_url.current(),
                    "a2a interface started"
                );
                a2a_handle = Some(handle);
                a2a_shutdown_tx = Some(tx);
                a2a_card_state = Some(card_state);
                a2a_public_url = Some(public_url);
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to start the a2a interface; it will not run this session");
            }
        }
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
        a2a_public_url,
    }
}

/// Build the live agent card, task store, session executor, and spawn the
/// A2A listener task. Shared between initial startup and reload: both
/// rebuild the listener from scratch when `[a2a]` changes, since a
/// visibility flip and a new base URL both need a fresh card as well as a
/// fresh listener.
///
/// Also spawns a background task that reloads the card whenever the tunnel's
/// status changes (so the card's public URL reflects it as soon as it
/// connects), and starts the restart-continuation sweep for tasks left
/// `Submitted`/`Working` from a previous run of this process.
///
/// # Errors
/// Returns an error if the persistent task store's directory can't be
/// created or read.
pub(crate) async fn build_a2a_listener(
    cfg: &Config,
    deps: A2aListenerDeps,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<(
    tokio::task::JoinHandle<()>,
    crate::a2a::SharedCardState,
    crate::a2a::SharedA2aPublicUrl,
)> {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&cfg.workspace_dir);
    let tunnel_status = deps.tunnel_status_rx.borrow().clone();
    let card_runtime = crate::a2a::CardRuntime::from_config_and_tunnel(
        &cfg.a2a,
        &cfg.gateway.bind,
        &tunnel_status,
    );
    let card_state =
        crate::a2a::CardState::load_or_default(&layout.agent_card_json(), &card_runtime);
    let keys = crate::a2a::A2aKeys::new_shared(&cfg.config_dir);
    let public_url = crate::a2a::A2aPublicUrl::new(
        cfg.a2a.clone(),
        cfg.gateway.bind.clone(),
        deps.tunnel_status_rx.clone(),
    );

    let task_store = crate::a2a::FileTaskStore::load(&layout.a2a_tasks_dir()).await?;

    let executor = crate::a2a::SessionExecutor::new(
        deps.agent_messenger,
        deps.session_registry,
        deps.bus_handle,
        deps.skill_state,
        Arc::clone(&card_state),
        layout.agent_inbox_dir(),
        cfg.timezone,
    );
    let inner = a2a_server::DefaultRequestHandler::new(
        executor,
        crate::a2a::DelegatingTaskStore(Arc::clone(&task_store)),
    )
    .with_capabilities(a2a::AgentCapabilities {
        streaming: Some(true),
        push_notifications: Some(false),
        extensions: None,
        extended_agent_card: None,
    });
    let handler = Arc::new(crate::a2a::ResiduumA2aHandler::new(
        inner,
        Arc::clone(&task_store),
    ));

    let resume_handler = Arc::clone(&handler);
    let resume_store = Arc::clone(&task_store);
    let mut sessions_ready = deps.sessions_ready.clone();
    tokio::spawn(async move {
        if sessions_ready.wait_for(|ready| *ready).await.is_err() {
            tracing::error!(
                "session spawner never became ready; a2a tasks left in progress were not resumed"
            );
            return;
        }
        crate::a2a::resume_in_progress_tasks(resume_handler, resume_store).await;
    });

    let listener = crate::a2a::A2aListener::new(
        cfg.a2a.clone(),
        cfg.gateway.bind.clone(),
        handler,
        Arc::clone(&card_state),
        keys,
        Arc::new(|| Some(Arc::<str>::from(crate::tunnel::tunnel_nonce()))),
        shutdown_rx.clone(),
    );
    let handle = crate::util::spawn_monitored("a2a", async move {
        if let Err(e) = listener.start().await {
            tracing::error!(error = %e, "a2a interface failed");
        }
    });

    spawn_a2a_card_tunnel_watcher(
        Arc::clone(&card_state),
        layout.agent_card_json(),
        cfg.a2a.clone(),
        cfg.gateway.bind.clone(),
        deps.tunnel_status_rx,
        shutdown_rx,
    );

    Ok((handle, card_state, public_url))
}

/// Reload the agent card whenever the tunnel's status changes, so its public
/// URL picks up a newly connected (or disconnected) tunnel without waiting
/// for a workspace or config reload. Stops when `shutdown_rx` fires, the
/// same signal that stops the listener this watcher was spawned alongside.
fn spawn_a2a_card_tunnel_watcher(
    card_state: crate::a2a::SharedCardState,
    card_path: std::path::PathBuf,
    a2a_cfg: crate::config::A2aConfig,
    gateway_bind: String,
    mut tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    crate::util::spawn_monitored("a2a-card-tunnel-watch", async move {
        loop {
            tokio::select! {
                changed = tunnel_status_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let status = tunnel_status_rx.borrow().clone();
                    let runtime = crate::a2a::CardRuntime::from_config_and_tunnel(&a2a_cfg, &gateway_bind, &status);
                    if let Err(e) = card_state.reload(&card_path, &runtime) {
                        tracing::warn!(error = %e, "failed to refresh the a2a agent card after a tunnel status change");
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        return;
                    }
                }
            }
        }
    });
}
