use std::sync::Arc;

use crate::types::GatewayState;
use residuum_background::registry::SessionRegistry;
use residuum_config::Config;

use crate::web;
use crate::ws::ws_handler;
use residuum_interfaces::AdapterSenders;

/// State bundle for the smaller, cross-cutting API routers, grouped so
/// [`build_gateway_app`] doesn't grow another positional parameter for each
/// one (and trip `clippy::too_many_arguments`).
pub struct ExtraApiStates {
    pub memory: web::memory::MemoryApiState,
    pub model: web::model::ModelApiState,
    pub a2a_agents: web::a2a::A2aAgentsStatusState,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<residuum_net::tunnel::TunnelStatus>,
    pub model_catalog: residuum_config::SharedModelCatalog,
    pub build: residuum_core::build_info::BuildInfo,
}

struct FeatureRouters {
    webhook: axum::Router,
    sessions: axum::Router,
    scheduled: axum::Router,
    agent_inbox: axum::Router,
    memory: axum::Router,
    model: axum::Router,
    a2a_agents: axum::Router,
}

fn build_feature_routers(state: &GatewayState, extra: ExtraApiStates) -> FeatureRouters {
    let webhook = axum::Router::new()
        .route(
            "/webhook/{name}",
            axum::routing::post(residuum_interfaces::webhook::webhook_handler),
        )
        .with_state(residuum_interfaces::webhook::WebhookState {
            publisher: state.publisher.clone(),
            webhooks: state.webhooks.clone(),
            tz: state.tz,
        });

    let sessions = web::sessions::sessions_api_router(web::sessions::SessionsApiState {
        registry: Arc::clone(&state.session_registry),
        store: Arc::clone(&state.session_store),
        tz: state.tz,
        messenger: Arc::clone(&state.agent_messenger),
        publisher: state.publisher.clone(),
        skill_state: Arc::clone(&state.skill_state),
        models: Arc::clone(&extra.model_catalog),
        main_conversation: state.layout.recent_messages_json(),
    });

    let scheduled = web::scheduled::scheduled_api_router(web::scheduled::ScheduledApiState {
        registry: Arc::clone(&state.session_registry),
        store: Arc::clone(&state.session_store),
        action_store: Arc::clone(&state.action_store),
        layout: state.layout.clone(),
        tz: state.tz,
        fires: Arc::clone(&state.pulse_fires),
    });

    FeatureRouters {
        webhook,
        sessions,
        scheduled,
        agent_inbox: web::inbox::agent_inbox_api_router(state.clone()),
        memory: web::memory::memory_api_router(extra.memory),
        model: web::model::model_api_router(extra.model),
        a2a_agents: web::a2a::a2a_agents_status_router(extra.a2a_agents),
    }
}

pub fn build_gateway_app(
    state: GatewayState,
    config_api_state: residuum_hub::web::ConfigApiState,
    extra: ExtraApiStates,
) -> axum::Router {
    use axum::routing::get;

    let state_tunnel_status_rx = extra.tunnel_status_rx.clone();
    let build = extra.build;
    let live_updates = state.workspace_watch_health.clone();
    let file_registry = state.file_registry.clone();
    let routers = build_feature_routers(&state, extra);

    axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .merge(routers.webhook)
        .merge(routers.sessions)
        .merge(routers.scheduled)
        .merge(routers.agent_inbox)
        .merge(routers.memory)
        .merge(routers.model)
        .merge(routers.a2a_agents)
        .merge(web::a2a::a2a_status_router(web::a2a::A2aStatusState {
            config: config_api_state.clone(),
            tunnel_status_rx: state_tunnel_status_rx,
        }))
        .merge(residuum_hub::web::agent_files_api_router(
            residuum_hub::web::AgentFilesState::running(&config_api_state, file_registry),
        ))
        .merge(web::agent_status_api_router(web::AgentStatusState {
            build,
            config: config_api_state,
            live_updates,
        }))
}

pub fn spawn_adapters(
    cfg: &Config,
    senders: &AdapterSenders,
    tz: chrono_tz::Tz,
    activity: &Arc<residuum_hub::activity::ActivityTracker>,
) -> crate::chat_adapters::ChatAdapters {
    let mut chat = crate::chat_adapters::ChatAdapters::new();
    if let Some(ref discord_cfg) = cfg.discord {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = residuum_interfaces::discord::DiscordInterface::new(
            discord_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        chat.insert(
            "discord",
            residuum_core::util::spawn_monitored("discord", async move {
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
        let iface = residuum_interfaces::telegram::TelegramInterface::new(
            telegram_cfg.clone(),
            senders.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        chat.insert(
            "telegram",
            residuum_core::util::spawn_monitored("telegram", async move {
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
        let iface = residuum_interfaces::teams::TeamsInterface::new(
            teams_cfg.clone(),
            senders.clone(),
            cfg.gateway.bind.clone(),
            cfg.workspace_dir.clone(),
            tz,
            rx,
        );
        let port = teams_cfg.port;
        let activity = Arc::clone(activity);
        chat.insert(
            "teams",
            residuum_core::util::spawn_monitored("teams", async move {
                run_teams_adapter(iface, port, &activity).await;
            }),
            tx,
        );
    }

    chat
}

pub(crate) async fn run_teams_adapter(
    iface: residuum_interfaces::teams::TeamsInterface,
    port: u16,
    activity: &residuum_hub::activity::ActivityTracker,
) {
    if let Err(e) = iface.start().await {
        tracing::error!(error = %format!("{e:#}"), port, "teams interface failed");
        activity.hub_notice(
            residuum_hub::types::NoticeLevel::Warn,
            format!(
                "The Teams adapter couldn't start on port {port} ({e:#}). Check that no other program or agent uses that port, or give this agent a different Teams port, then restart it."
            ),
        );
    }
}

pub(crate) struct A2aServingDeps {
    pub session_registry: Arc<SessionRegistry>,
    pub agent_messenger: Arc<residuum_background::messaging::AgentMessenger>,
    pub skill_state: residuum_skills::SharedSkillState,
    pub bus_handle: residuum_bus::BusHandle,
    pub sessions_ready: tokio::sync::watch::Receiver<bool>,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<residuum_net::tunnel::TunnelStatus>,
}

pub(crate) async fn build_agent_a2a(
    cfg: &Config,
    deps: A2aServingDeps,
) -> Option<residuum_a2a::AgentA2a> {
    let layout = residuum_config::WorkspaceLayout::new(&cfg.workspace_dir);
    match residuum_a2a::agent_a2a_router(residuum_a2a::AgentA2aState {
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
        tunnel_status_rx: deps.tunnel_status_rx,
    })
    .await
    {
        Ok(mut agent) => {
            agent.router = super::agent_span_layer(&cfg.agent_name, agent.router);
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
