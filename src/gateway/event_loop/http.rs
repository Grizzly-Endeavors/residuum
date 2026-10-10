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
    /// The relay tunnel's status, for the A2A status route's relay address.
    pub tunnel_status_rx: tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
}

/// The agent-scoped routers [`build_gateway_app`] merges in, built once by
/// [`build_feature_routers`] so the top-level function stays under the
/// line-count lint.
struct FeatureRouters {
    webhook: axum::Router,
    file: axum::Router,
    sessions: axum::Router,
    scheduled: axum::Router,
    agent_inbox: axum::Router,
    memory: axum::Router,
    model: axum::Router,
    a2a_agents: axum::Router,
}

/// Build every feature router [`build_gateway_app`] merges onto the base
/// `/ws` router. Split out purely to keep that function's line count under
/// the lint threshold — each router here is independent and self-contained.
fn build_feature_routers(state: &GatewayState, extra: ExtraApiStates) -> FeatureRouters {
    use axum::routing::get;

    // Always mounted: the table is swapped on reload, so webhooks added later
    // work without rebinding the server. Unknown names get a 404. The hub
    // serves it at `/webhook/{agent}/{name}`.
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

    FeatureRouters {
        webhook,
        file,
        sessions,
        scheduled,
        agent_inbox: web::inbox::agent_inbox_api_router(state.clone()),
        memory: web::memory::memory_api_router(extra.memory),
        model: web::model::model_api_router(extra.model),
        a2a_agents: web::a2a::a2a_agents_status_router(extra.a2a_agents),
    }
}

/// Build one running agent's router, rooted at `/`: its WebSocket, webhooks,
/// files, sessions, scheduled work, inbox, memory search, model completion,
/// A2A settings, and status. The chat history, usage, user inbox, and A2A
/// settings routes are the same file routes a stopped agent gets from
/// `hub::AgentDirectory::agent_file_router`, here on the agent's own open
/// state. It carries no hub-level route, no repair route (see
/// `hub::http::agent_repair_router`), no static assets and no request guards;
/// the hub router serves it under `/api/agents/{name}/` and applies those.
pub fn build_gateway_app(
    state: GatewayState,
    config_api_state: web::ConfigApiState,
    extra: ExtraApiStates,
) -> axum::Router {
    use axum::routing::get;

    let state_tunnel_status_rx = extra.tunnel_status_rx.clone();
    let live_updates = state.workspace_watch_health.clone();
    let routers = build_feature_routers(&state, extra);

    axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .merge(routers.webhook)
        .merge(routers.file)
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
        .merge(web::agent_files_api_router(web::AgentFilesState::from(
            &config_api_state,
        )))
        .merge(web::agent_status_api_router(web::AgentStatusState {
            config: config_api_state,
            live_updates,
        }))
}

/// Spawn the Discord, Telegram, and Teams adapters that are configured.
pub fn spawn_adapters(
    cfg: &Config,
    senders: &AdapterSenders,
    tz: chrono_tz::Tz,
    activity: &Arc<crate::hub::activity::ActivityTracker>,
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
        let port = teams_cfg.port;
        let activity = Arc::clone(activity);
        chat.insert(
            "teams",
            crate::util::spawn_monitored("teams", async move {
                run_teams_adapter(iface, port, &activity).await;
            }),
            tx,
        );
    }

    chat
}

/// Run the Teams adapter until it stops. A failure to start (most often a
/// port another program or agent already holds) is logged and told to the
/// user as a hub notice naming the agent and the port.
pub(crate) async fn run_teams_adapter(
    iface: crate::interfaces::teams::TeamsInterface,
    port: u16,
    activity: &crate::hub::activity::ActivityTracker,
) {
    if let Err(e) = iface.start().await {
        tracing::error!(error = %format!("{e:#}"), port, "teams interface failed");
        activity.hub_notice(
            crate::hub::types::NoticeLevel::Warn,
            format!(
                "The Teams adapter couldn't start on port {port} ({e:#}). Check that no other program or agent uses that port, or give this agent a different Teams port, then restart it."
            ),
        );
    }
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
    /// The relay tunnel's status, so the agent's card advertises its relay
    /// address while the tunnel is connected.
    pub tunnel_status_rx: tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
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
        tunnel_status_rx: deps.tunnel_status_rx,
    })
    .await
    {
        Ok(mut agent) => {
            // Its handlers run under the hub's A2A listener, outside any agent
            // span, so the router carries the span itself.
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
