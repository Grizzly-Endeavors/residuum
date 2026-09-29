//! HTTP server setup and adapter spawning in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::background::registry::SessionRegistry;
use crate::config::Config;
use crate::gateway::types::{GatewayState, ServerCommand, StopRequest};

use crate::gateway::web;
use crate::gateway::ws::ws_handler;

/// Bundled senders for spawning a chat adapter (Discord or Telegram).
#[derive(Clone)]
pub struct AdapterSenders {
    pub publisher: crate::bus::Publisher,
    pub bus_handle: crate::bus::BusHandle,
    pub reload: crate::gateway::types::ReloadSender,
    pub command: mpsc::Sender<ServerCommand>,
    pub stop: mpsc::Sender<StopRequest>,
    /// Looked up when a `/stop` command targets a conversation session
    /// rather than main — see [`crate::interfaces::dispatch_stop_request`].
    pub session_registry: Arc<SessionRegistry>,
    /// Where the adapter registers the conversations it can reach.
    pub(crate) conversations: crate::interfaces::conversations::ConversationDirectory,
}

/// State bundle for the smaller, cross-cutting API routers, grouped so
/// [`build_gateway_app`] doesn't grow another positional parameter for each
/// one (and trip `clippy::too_many_arguments`).
pub struct ExtraApiStates {
    pub memory: web::memory::MemoryApiState,
    pub model: web::model::ModelApiState,
    pub a2a_agents: web::a2a::A2aAgentsStatusState,
}

/// Cloud connection routes. Disconnect is refused over the tunnel, since a
/// remote disconnect leaves no way to reconnect.
fn cloud_api_router(state: &GatewayState, config_api_state: &web::ConfigApiState) -> axum::Router {
    use axum::routing::{get, post};

    let cloud_state = web::cloud::CloudApiState {
        hub_dir: config_api_state.hub_dir.clone(),
        reload_tx: state.reload_tx.clone(),
        tunnel_status_rx: state.tunnel_status_rx.clone(),
        secret_lock: Arc::clone(&config_api_state.secret_lock),
    };
    axum::Router::new()
        .route("/api/cloud/status", get(web::cloud::api_cloud_status))
        .route("/cloud/callback", get(web::cloud::cloud_callback))
        .with_state(cloud_state.clone())
        .merge(
            axum::Router::new()
                .route(
                    "/api/cloud/disconnect",
                    post(web::cloud::api_cloud_disconnect),
                )
                .route_layer(axum::middleware::from_fn(
                    crate::gateway::remote_control_guard::reject_remote_shutdown_and_disconnect,
                ))
                .with_state(cloud_state),
        )
}

/// Update and shutdown routes. Shutdown is refused over the tunnel, since a
/// remote shutdown leaves no way to bring the gateway back.
fn update_api_router(update_api_state: web::update::UpdateApiState) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/api/update/status", get(web::update::api_update_status))
        .route("/api/update/check", post(web::update::api_update_check))
        .route("/api/update/apply", post(web::update::api_update_apply))
        .route("/api/update/restart", post(web::update::api_update_restart))
        .with_state(update_api_state.clone())
        .merge(
            axum::Router::new()
                .route("/api/shutdown", post(web::update::api_shutdown))
                .route_layer(axum::middleware::from_fn(
                    crate::gateway::remote_control_guard::reject_remote_shutdown_and_disconnect,
                ))
                .with_state(update_api_state),
        )
}

/// The smaller, cross-cutting API routers [`build_gateway_app`] merges in,
/// built once by [`build_feature_routers`] so the top-level function stays
/// under the line-count lint.
struct FeatureRouters {
    webhook: axum::Router,
    cloud: axum::Router,
    update: axum::Router,
    tracing: axum::Router,
    file: axum::Router,
    sessions: axum::Router,
    scheduled: axum::Router,
    workbench: axum::Router,
    checkpoints: axum::Router,
    agent_inbox: axum::Router,
    memory: axum::Router,
    model: axum::Router,
    a2a_agents: axum::Router,
}

/// Build every feature router [`build_gateway_app`] merges onto the base
/// `/ws` router. Split out purely to keep that function's line count under
/// the lint threshold — each router here is independent and self-contained.
fn build_feature_routers(
    state: &GatewayState,
    config_api_state: &web::ConfigApiState,
    update_api_state: web::update::UpdateApiState,
    tracing_api_state: web::tracing_api::TracingApiState,
    workbench_serving: crate::workbench::server::WorkbenchServing,
    extra: ExtraApiStates,
) -> FeatureRouters {
    use axum::routing::get;

    // Always mounted: the table is swapped on reload, so webhooks added later
    // work without rebinding the server. Unknown names get a 404.
    let webhook = axum::Router::new()
        .route(
            "/webhook/{name}",
            axum::routing::post(crate::interfaces::webhook::webhook_handler),
        )
        .with_state(crate::interfaces::webhook::WebhookState {
            publisher: state.publisher.clone(),
            webhooks: state.webhooks.clone(),
            tz: state.tz,
        });

    let cloud = cloud_api_router(state, config_api_state);
    let update = update_api_router(update_api_state);

    let tracing = tracing_api_router(tracing_api_state);

    let file = axum::Router::new()
        .route(
            "/api/files/{id}",
            get(crate::gateway::file_server::serve_file),
        )
        .route(
            "/api/files/workspace",
            get(crate::gateway::file_server::serve_workspace_file),
        )
        .with_state(state.file_registry.clone());

    let sessions = web::sessions::sessions_api_router(web::sessions::SessionsApiState {
        registry: Arc::clone(&state.session_registry),
        store: Arc::clone(&state.session_store),
        tz: state.tz,
        messenger: Arc::clone(&state.agent_messenger),
        publisher: state.publisher.clone(),
        skill_state: Arc::clone(&state.skill_state),
    });

    let scheduled = web::scheduled::scheduled_api_router(web::scheduled::ScheduledApiState {
        registry: Arc::clone(&state.session_registry),
        store: Arc::clone(&state.session_store),
        action_store: Arc::clone(&state.action_store),
        layout: state.layout.clone(),
        tz: state.tz,
    });

    let workbench = web::workbench::workbench_api_router(web::workbench::WorkbenchApiState {
        dir: crate::workspace::layout::WorkspaceLayout::new(&config_api_state.workspace_dir)
            .team()
            .workbench_dir(),
        serving: workbench_serving,
        tunnel_status_rx: state.tunnel_status_rx.clone(),
        checkpoints: Arc::clone(&config_api_state.checkpoints),
        team: config_api_state.team.clone(),
    });

    let checkpoints =
        web::checkpoints::checkpoints_api_router(web::checkpoints::CheckpointApiState {
            checkpoints: Arc::clone(&config_api_state.checkpoints),
        });

    FeatureRouters {
        webhook,
        cloud,
        update,
        tracing,
        file,
        sessions,
        scheduled,
        workbench,
        checkpoints,
        agent_inbox: web::inbox::agent_inbox_api_router(state.clone()),
        memory: web::memory::memory_api_router(extra.memory),
        model: web::model::model_api_router(extra.model),
        a2a_agents: web::a2a::a2a_agents_status_router(extra.a2a_agents),
    }
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
    use axum::routing::get;

    let routers = build_feature_routers(
        &state,
        &config_api_state,
        update_api_state,
        tracing_api_state,
        workbench_serving,
        extra,
    );

    axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .merge(routers.webhook)
        .merge(routers.file)
        .merge(routers.sessions)
        .merge(routers.scheduled)
        .merge(routers.cloud)
        .merge(routers.update)
        .merge(routers.tracing)
        .merge(routers.workbench)
        .merge(routers.checkpoints)
        .merge(routers.agent_inbox)
        .merge(routers.memory)
        .merge(routers.model)
        .merge(routers.a2a_agents)
        .merge(web::a2a::a2a_status_router(config_api_state.clone()))
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

/// Spawn the Discord, Telegram, and Teams adapters that are configured.
pub fn spawn_adapters(
    cfg: &Config,
    senders: &AdapterSenders,
    tz: chrono_tz::Tz,
) -> crate::gateway::chat_adapters::ChatAdapters {
    let mut chat = crate::gateway::chat_adapters::ChatAdapters::new();
    if let Some(ref discord_cfg) = cfg.discord {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::discord::DiscordInterface::new(
            discord_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        chat.insert(
            "discord",
            crate::util::spawn_monitored("discord", async move {
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "discord interface failed");
                }
            }),
            tx,
        );
        tracing::info!("discord interface started");
    }

    if let Some(ref telegram_cfg) = cfg.telegram {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::telegram::TelegramInterface::new(
            telegram_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        chat.insert(
            "telegram",
            crate::util::spawn_monitored("telegram", async move {
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "telegram interface failed");
                }
            }),
            tx,
        );
        tracing::info!("telegram interface started");
    }

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
        chat.insert(
            "teams",
            crate::util::spawn_monitored("teams", async move {
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "teams interface failed");
                }
            }),
            tx,
        );
    }

    chat
}

/// What [`build_agent_a2a`] needs beyond `Config`: the agent's live runtime
/// state (sessions, messaging, skills, and the bus) rather than anything
/// derivable from config alone.
pub(crate) struct A2aServingDeps {
    pub session_registry: Arc<SessionRegistry>,
    pub agent_messenger: Arc<crate::background::messaging::AgentMessenger>,
    pub skill_state: crate::skills::SharedSkillState,
    pub bus_handle: crate::bus::BusHandle,
    /// Turns `true` once the session spawn listener is subscribed. Restart
    /// continuations wait on it: a session spawn published before then has
    /// no listener and would be lost.
    pub sessions_ready: tokio::sync::watch::Receiver<bool>,
}

/// Build the agent's A2A server with [`crate::a2a::agent_a2a_router`]. The
/// hub's A2A listener serves the router at `/agents/<name>/`.
///
/// Shared between startup and reload: both rebuild the agent's A2A state from
/// scratch when `[a2a]` changes, since a visibility flip and a new base URL
/// both need a fresh card. A build failure is logged and reported as `None`:
/// the rest of the agent still runs, and the next successful reload retries.
pub(crate) async fn build_agent_a2a(
    cfg: &Config,
    deps: A2aServingDeps,
) -> Option<crate::a2a::AgentA2a> {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&cfg.workspace_dir);
    match crate::a2a::agent_a2a_router(crate::a2a::AgentA2aState {
        name: cfg.agent_name.clone(),
        a2a: cfg.a2a.clone(),
        bind: cfg.gateway.bind.clone(),
        layout,
        timezone: cfg.timezone,
        agent_messenger: deps.agent_messenger,
        session_registry: deps.session_registry,
        bus_handle: deps.bus_handle,
        skill_state: deps.skill_state,
        sessions_ready: deps.sessions_ready,
    })
    .await
    {
        Ok(agent) => {
            tracing::info!(
                visibility = %cfg.a2a.visibility,
                public_url = %agent.public_url,
                "a2a interface ready"
            );
            Some(agent)
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to build the a2a interface; it will not run until the next successful reload");
            None
        }
    }
}
