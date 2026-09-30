//! One agent's start-up and main event loop.
//!
//! [`start_agent`] initializes an agent's subsystems on the hub's shared
//! services and returns its [`AgentRuntime`] plus the [`AgentControl`] the
//! hub keeps: the agent's HTTP and A2A routers, its reload channel, and its
//! stop signal. [`run_agent_loop`] is the `tokio::select` loop that processes
//! all of the agent's events until the hub stops it.

use std::sync::Arc;

use tokio::time::Duration;
use tracing::Instrument;

use crate::config::{Config, HubConfig};
use crate::gateway::types::{
    AgentRuntime, CoreReceivers, GatewayCore, GatewayState, ReloadSender, ReloadSignal,
};
use crate::hub::activity::ActivityTracker;
use crate::hub::services::HubServices;
use crate::pulse::scheduler::PulseScheduler;
use crate::util::FatalError;

use super::commands::handle_server_command;
use super::http::{
    A2aServingDeps, AdapterSenders, build_agent_a2a, build_gateway_app, spawn_adapters,
};
use super::pulse::{handle_pulse_result_event, handle_pulse_tick};
use super::turns::handle_inbound_message;

use crate::gateway::{actions, idle, last_known_good, reload, watcher, web};

/// What the hub gives [`start_agent`].
pub(crate) struct AgentStartInputs {
    /// The agent's name.
    pub name: String,
    /// The agent's directory (its workspace root).
    pub agent_dir: std::path::PathBuf,
    /// Services shared by every agent.
    pub services: HubServices,
    /// The hub config the agent resolves its own config against.
    pub hub_cfg: HubConfig,
    /// Where the agent reports main-conversation activity.
    pub activity: Arc<ActivityTracker>,
}

/// How the hub reaches a started agent.
#[derive(Clone)]
pub(crate) struct AgentControl {
    /// The agent's HTTP routes, rooted at `/`.
    pub router: axum::Router,
    /// The agent's current A2A router; `None` while A2A is disabled for it.
    /// Changes when a reload rebuilds the agent's A2A state.
    pub a2a_router: tokio::sync::watch::Receiver<Option<axum::Router>>,
    /// Delivers config reload signals to the agent's event loop.
    pub reload_tx: ReloadSender,
    /// Asks the agent's event loop to stop.
    pub stop_tx: tokio::sync::mpsc::Sender<()>,
    /// Counts config reloads the agent has finished, so the hub can wait for
    /// a reload it triggered and notice changes made behind its back.
    pub reload_done: tokio::sync::watch::Receiver<u64>,
    /// What the hub cleans up when the event loop dies without shutting the
    /// agent down.
    pub cleanup: AgentCleanup,
    /// The agent's live sessions, for the hub's bug reports.
    pub session_registry: Arc<crate::background::registry::SessionRegistry>,
    /// The agent's bus publisher, for handing the agent a message.
    publisher: crate::bus::Publisher,
}

impl AgentControl {
    /// Deliver `content` to the agent's main conversation as a message from
    /// `from`: the owner's own message for the user, or a message from the
    /// creating agent's address. An idle main starts a turn on it.
    ///
    /// # Errors
    /// Returns a plain-language reason when the message could not be
    /// published on the agent's bus.
    pub(crate) async fn deliver_to_main(
        &self,
        from: &crate::hub::Actor,
        content: String,
        tz: chrono_tz::Tz,
    ) -> Result<(), String> {
        let event = match from {
            crate::hub::Actor::User => crate::bus::MessageEvent {
                id: format!("hub-{}", uuid::Uuid::new_v4()),
                content,
                origin: crate::interfaces::types::MessageOrigin {
                    endpoint: "ws".to_string(),
                    sender: None,
                    conversation: None,
                    agent_sender: None,
                },
                timestamp: crate::time::now_local(tz),
                images: Vec::new(),
                context: None,
            },
            crate::hub::Actor::Agent(creator) => {
                crate::bus::MessageEvent::from_agent(&crate::bus::AgentMessageEvent {
                    from: crate::bus::SessionAddress::from(format!("agent:{creator}")),
                    from_category: "agent".to_string(),
                    content,
                    hop_count: 0,
                })
            }
        };
        self.publisher
            .publish(crate::bus::topics::UserMessage, event)
            .await
            .map_err(|e| format!("the agent's message channel is closed: {e}"))
    }
}

/// The parts of an agent that outlive a crashed event loop and must be
/// stopped explicitly: its live sessions and the MCP servers it spawned.
#[derive(Clone)]
pub(crate) struct AgentCleanup {
    sessions: Arc<crate::background::SessionRuntime>,
    mcp_registry: crate::mcp::SharedMcpRegistry,
}

impl AgentCleanup {
    /// Stop the agent's live sessions, recording them as interrupted the way
    /// a normal stop does, and disconnect its MCP servers.
    pub(crate) async fn run(&self) {
        self.sessions.shutdown(SESSION_SHUTDOWN_TIMEOUT).await;
        self.mcp_registry.write().await.disconnect_all().await;
    }
}

/// A started agent, ready to run.
pub(crate) struct StartedAgent {
    /// The state [`run_agent_loop`] consumes.
    pub runtime: AgentRuntime,
    /// The hub's handle on the agent.
    pub control: AgentControl,
}

/// How an agent's event loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentExit {
    /// The hub asked the agent to stop and it shut down cleanly.
    Stopped,
    /// The loop's own inbound message channel closed underneath it.
    BusClosed,
}

/// Initialize the agent at `inputs.agent_dir` on the hub's shared services,
/// spawn its adapters, watchers, and A2A serving state, and return it ready
/// for [`run_agent_loop`].
///
/// If the agent's live config files fail to load, or load but the agent
/// can't actually start on them (no usable main provider, etc.), the agent
/// falls back to its own last-known-good copy (see [`last_known_good`])
/// instead of refusing to start. A fallback is announced with a notice once
/// the agent is up, so it's visible to anyone using the web UI.
///
/// # Errors
///
/// Returns `FatalError` if the config can't be loaded and no working
/// last-known-good copy is available, or an essential subsystem can't start.
pub(crate) async fn start_agent(inputs: AgentStartInputs) -> Result<StartedAgent, FatalError> {
    let AgentStartInputs {
        name,
        agent_dir,
        services,
        hub_cfg,
        activity,
    } = inputs;
    let (core, receivers) = GatewayCore::new(agent_dir.join("config"), services.hub_dir.clone());
    let (cfg, hub_cfg, parts, fallback_problem) =
        load_and_initialize(&agent_dir, hub_cfg, &core.publisher, &services).await?;

    let (agent_stop_tx, agent_stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (model_call_resources_tx, model_call_resources_rx) = tokio::sync::watch::channel(Arc::new(
        web::model::ModelCallResources::from_spawn_context(&parts.spawn_context),
    ));
    let (a2a_router_tx, a2a_router_rx) = tokio::sync::watch::channel(None);
    let (reload_done_tx, reload_done_rx) = tokio::sync::watch::channel(0_u64);

    let spawned = spawn_agent_tasks(
        &core,
        &parts,
        &cfg,
        &services,
        &activity,
        model_call_resources_rx,
    )
    .await?;

    if let Some(problem) = fallback_problem {
        tracing::error!(
            error = %problem,
            "startup fell back to the last-known-good config"
        );
        crate::gateway::helpers::publish_notice(
            &core.publisher,
            format!(
                "residuum couldn't start using your current config ({problem}). It's running on the last configuration that worked instead — fix the files above, then reload (or restart residuum) to apply your changes."
            ),
        )
        .await;
    } else {
        last_known_good::save(&cfg.config_dir);
    }

    let control = AgentControl {
        router: agent_span_layer(&name, spawned.router.clone()),
        a2a_router: a2a_router_rx,
        reload_tx: core.reload_tx.clone(),
        stop_tx: agent_stop_tx,
        reload_done: reload_done_rx,
        cleanup: AgentCleanup {
            sessions: Arc::clone(&parts.session_runtime),
            mcp_registry: Arc::clone(&parts.mcp_registry),
        },
        session_registry: Arc::clone(&parts.session_registry),
        publisher: core.publisher.clone(),
    };
    let sibling_fanout = Arc::clone(&services.sibling_fanout);
    let runtime = build_runtime(
        parts,
        core,
        receivers,
        cfg,
        hub_cfg,
        spawned,
        RuntimeChannels {
            name,
            services,
            activity,
            agent_stop_rx,
            model_call_resources_tx,
            a2a_router_tx,
            reload_done_tx,
        },
    )
    .await?;
    // The hub's one discovery task feeds every registered agent's client hub;
    // this agent joins it until it stops.
    sibling_fanout
        .register(&runtime.name, Arc::clone(&runtime.a2a_hub))
        .await;
    Ok(StartedAgent { runtime, control })
}

/// Wrap `router` so every request it handles runs inside the agent's root
/// span, giving handler logs and the tasks they spawn the `agent` field.
pub(crate) fn agent_span_layer(name: &str, router: axum::Router) -> axum::Router {
    let span = agent_span(name);
    router.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let span = span.clone();
            async move { next.run(request).instrument(span).await }
        },
    ))
}

/// Load the agent's config against `hub_cfg`, initializing it on that, and
/// falling back to the agent's last-known-good copy if either fails. Returns
/// the configs actually used, their initialized components, and — only when
/// a fallback was used — a description of what was wrong, for the caller to
/// publish once the agent is up.
async fn load_and_initialize(
    agent_dir: &std::path::Path,
    hub_cfg: HubConfig,
    publisher: &crate::bus::Publisher,
    services: &HubServices,
) -> Result<
    (
        Config,
        HubConfig,
        crate::gateway::startup::GatewayComponents,
        Option<String>,
    ),
    FatalError,
> {
    match Config::load_agent_at(agent_dir, &hub_cfg) {
        Ok(cfg) => {
            match crate::gateway::startup::initialize(&cfg, &hub_cfg, publisher, services).await {
                Ok(parts) => Ok((cfg, hub_cfg, parts, None)),
                Err(err) => {
                    agent_last_known_good_fallback(agent_dir, hub_cfg, publisher, services, err)
                        .await
                }
            }
        }
        Err(err) => {
            agent_last_known_good_fallback(agent_dir, hub_cfg, publisher, services, err).await
        }
    }
}

/// Try starting the agent from its last-known-good config after
/// `original_err` broke the live files. Returns `original_err` untouched if
/// there's no last-known-good copy, or it fails to initialize too — the
/// live files are still the more relevant problem to report in that case.
async fn agent_last_known_good_fallback(
    agent_dir: &std::path::Path,
    hub_cfg: HubConfig,
    publisher: &crate::bus::Publisher,
    services: &HubServices,
    original_err: FatalError,
) -> Result<
    (
        Config,
        HubConfig,
        crate::gateway::startup::GatewayComponents,
        Option<String>,
    ),
    FatalError,
> {
    let Ok(lkg_cfg) = last_known_good::load(agent_dir, &hub_cfg) else {
        return Err(original_err);
    };
    match crate::gateway::startup::initialize(&lkg_cfg, &hub_cfg, publisher, services).await {
        Ok(parts) => {
            let problem = original_err.to_string();
            Ok((lkg_cfg, hub_cfg, parts, Some(problem)))
        }
        Err(_lkg_init_err) => Err(original_err),
    }
}

/// Handles returned from spawning the agent's router, adapters, and watchers.
struct SpawnedHandles {
    router: axum::Router,
    adapters: crate::gateway::chat_adapters::ChatAdapters,
    a2a: Option<crate::a2a::AgentA2a>,
    webhooks: crate::interfaces::webhook::WebhookTable,
    watcher_handle: Option<tokio::task::JoinHandle<()>>,
    root_config_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    workbench_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    change_feed_handle: Option<tokio::task::JoinHandle<()>>,
    /// Raised by [`build_runtime`] once the session spawn listener is up.
    sessions_ready_tx: tokio::sync::watch::Sender<bool>,
}

/// Bundle of `*ApiState` values `build_gateway_app` needs, split out of
/// `spawn_agent_tasks` to keep it under the line-count lint.
struct ApiStates {
    config: web::ConfigApiState,
    memory: web::memory::MemoryApiState,
    model: web::model::ModelApiState,
}

fn build_api_states(
    cfg: &Config,
    parts: &crate::gateway::startup::GatewayComponents,
    core: &GatewayCore,
    model_call_resources_rx: tokio::sync::watch::Receiver<Arc<web::model::ModelCallResources>>,
) -> ApiStates {
    ApiStates {
        config: web::ConfigApiState {
            hub_dir: core.hub_dir.clone(),
            config_dir: cfg.config_dir.clone(),
            agent_name: cfg.agent_name.clone(),
            workspace_dir: parts.layout.root().to_path_buf(),
            memory_dir: Some(parts.layout.memory_dir()),
            reload_tx: Some(core.reload_tx.clone()),
            scope: crate::gateway::web::WorkspaceScope::Agent,
            checkpoints: Arc::clone(&parts.checkpoints),
            team: Some(parts.team.view_for_user(parts.layout.root())),
        },
        memory: web::memory::MemoryApiState {
            hybrid_searcher: Arc::clone(&parts.hybrid_searcher),
        },
        model: web::model::ModelApiState {
            resources: model_call_resources_rx,
        },
    }
}

/// Build the `GatewayState` the agent's routes share, bundling the pieces
/// `spawn_agent_tasks` has already built or spawned by this point — split
/// out purely to keep that function under the line-count lint.
fn build_gateway_state(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    file_registry: &crate::gateway::file_server::FileRegistry,
    webhooks: &crate::interfaces::webhook::WebhookTable,
    workspace_watch_health: &tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
    services: &HubServices,
    activity: &Arc<ActivityTracker>,
) -> GatewayState {
    GatewayState {
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx.clone(),
        stop_tx: core.stop_tx.clone(),
        agent_inbox_dir: parts.layout.agent_inbox_dir(),
        tz: parts.tz,
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
        file_registry: file_registry.clone(),
        webhooks: webhooks.clone(),
        session_registry: Arc::clone(&parts.session_registry),
        session_store: Arc::clone(&parts.session_store),
        agent_messenger: Arc::clone(&parts.agent_messenger),
        skill_state: Arc::clone(&parts.skill_state),
        workspace_watch_health: workspace_watch_health.clone(),
        team_feed: Arc::clone(&services.team_feed),
        action_store: Arc::clone(&parts.action_store),
        layout: parts.layout.clone(),
        activity: Arc::clone(activity),
    }
}

/// The agent's A2A serving dependencies, plus the sender that marks the
/// session spawner ready (raised by [`build_runtime`] once it is subscribed).
fn a2a_serving_deps(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
) -> (A2aServingDeps, tokio::sync::watch::Sender<bool>) {
    let (sessions_ready_tx, sessions_ready_rx) = tokio::sync::watch::channel(false);
    let deps = A2aServingDeps {
        session_registry: Arc::clone(&parts.session_registry),
        agent_messenger: Arc::clone(&parts.agent_messenger),
        skill_state: Arc::clone(&parts.skill_state),
        bus_handle: core.bus_handle.clone(),
        sessions_ready: sessions_ready_rx,
    };
    (deps, sessions_ready_tx)
}

/// A config-file poller's task handle.
type WatcherHandle = Option<tokio::task::JoinHandle<()>>;

/// Spawn the config-file pollers: workspace files (`mcp.json`,
/// `channels.toml`, `agent-card.json`, `a2a.json`) signaling
/// `ReloadSignal::Workspace`, and root files (`config.toml`,
/// `providers.toml`) signaling `ReloadSignal::Agent`. The hub polls
/// `hub/config.toml` itself and tells each agent to reload.
fn spawn_config_watchers(
    cfg: &Config,
    parts: &crate::gateway::startup::GatewayComponents,
    core: &GatewayCore,
) -> (WatcherHandle, WatcherHandle) {
    let watcher_handle = Some(watcher::spawn_workspace_watcher(
        parts.layout.mcp_json(),
        parts.layout.channels_toml(),
        parts.layout.agent_card_json(),
        parts.layout.a2a_agents_json(),
        core.reload_tx.clone(),
    ));
    let root_config_watcher_handle = Some(watcher::spawn_root_config_watcher(
        cfg.config_dir.join("config.toml"),
        cfg.config_dir.join("providers.toml"),
        core.reload_tx.clone(),
    ));
    (watcher_handle, root_config_watcher_handle)
}

/// Spawn the agent's router, chat adapters, A2A serving state, and watchers.
async fn spawn_agent_tasks(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    cfg: &Config,
    services: &HubServices,
    activity: &Arc<ActivityTracker>,
    model_call_resources_rx: tokio::sync::watch::Receiver<Arc<web::model::ModelCallResources>>,
) -> Result<SpawnedHandles, FatalError> {
    let adapter_senders = AdapterSenders {
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
        reload: core.reload_tx.clone(),
        command: core.command_tx.clone(),
        stop: core.stop_tx.clone(),
        session_registry: Arc::clone(&parts.session_registry),
        conversations: parts.endpoint_registry.conversations().clone(),
    };

    let file_registry = crate::gateway::file_server::FileRegistry::new(cfg.agent_name.clone())
        .with_workspace_root(parts.layout.root().to_path_buf());
    file_registry.spawn_cleanup_task();
    let webhooks = crate::interfaces::webhook::WebhookTable::from_config(&cfg.webhooks);
    let ChangeFeeds {
        workbench_watcher: workbench_watcher_handle,
        workspace: change_feed_handle,
        health: workspace_watch_health,
    } = spawn_change_feed_tasks(core, &parts.layout, &services.team_feed).await;
    let state = build_gateway_state(
        core,
        parts,
        &file_registry,
        &webhooks,
        &workspace_watch_health,
        services,
        activity,
    );
    let api_states = build_api_states(cfg, parts, core, model_call_resources_rx);
    let router = build_gateway_app(
        state,
        api_states.config,
        super::http::ExtraApiStates {
            memory: api_states.memory,
            model: api_states.model,
            a2a_agents: web::a2a::A2aAgentsStatusState {
                hub: Arc::clone(&parts.a2a_hub),
                tracker: Arc::clone(&parts.a2a_tracker),
            },
        },
    );
    let (a2a_deps, sessions_ready_tx) = a2a_serving_deps(core, parts);
    let adapters = spawn_adapters(cfg, &adapter_senders, parts.tz);
    let a2a = if cfg.a2a.enabled {
        build_agent_a2a(cfg, a2a_deps).await
    } else {
        None
    };
    let (watcher_handle, root_config_watcher_handle) = spawn_config_watchers(cfg, parts, core);

    Ok(SpawnedHandles {
        router,
        adapters,
        a2a,
        webhooks,
        watcher_handle,
        root_config_watcher_handle,
        workbench_watcher_handle,
        change_feed_handle,
        sessions_ready_tx,
    })
}

/// The change-feed tasks the agent runs.
struct ChangeFeeds {
    /// The artifact reload watcher that follows the team feed.
    workbench_watcher: Option<tokio::task::JoinHandle<()>>,
    /// The feed over the agent's directory.
    workspace: Option<tokio::task::JoinHandle<()>>,
    /// Whether the agent-directory feed is running.
    health: tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
}

/// Start the change feed over the agent's directory and the artifact reload
/// watcher. The team directory has one feed for the whole hub
/// ([`TeamChangeFeed`](crate::hub::services::TeamChangeFeed)); the watcher
/// reads it and publishes its reloads on the agent's own bus.
async fn spawn_change_feed_tasks(
    core: &GatewayCore,
    layout: &crate::workspace::layout::WorkspaceLayout,
    team_feed: &crate::hub::services::TeamChangeFeed,
) -> ChangeFeeds {
    let (health_tx, health) =
        tokio::sync::watch::channel(crate::workspace::watch::WatchHealth::Starting);
    let workbench_watcher = match crate::workbench::watcher::spawn_workbench_watcher(
        layout.team().workbench_dir(),
        &team_feed.bus,
        core.publisher.clone(),
    )
    .await
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe the artifact reload watcher to the team change feed; open artifacts won't reload on their own");
            None
        }
    };
    let change_feed = crate::workspace::watch::spawn_change_feed(
        layout.root().to_path_buf(),
        None,
        core.publisher.clone(),
        health_tx,
    );
    ChangeFeeds {
        workbench_watcher,
        workspace: Some(change_feed),
        health,
    }
}

/// The agent's name, the hub-provided services, and the per-run channels
/// `build_runtime` needs, bundled to reduce its argument count.
struct RuntimeChannels {
    name: String,
    services: HubServices,
    activity: Arc<ActivityTracker>,
    agent_stop_rx: tokio::sync::mpsc::Receiver<()>,
    /// Pushed a fresh `ModelCallResources` on every config reload; stored on
    /// `AgentRuntime` so `crate::gateway::reload` can push to it.
    model_call_resources_tx: tokio::sync::watch::Sender<Arc<web::model::ModelCallResources>>,
    /// Publishes the agent's current A2A router to the hub.
    a2a_router_tx: tokio::sync::watch::Sender<Option<axum::Router>>,
    /// Counts finished config reloads for the hub.
    reload_done_tx: tokio::sync::watch::Sender<u64>,
}

/// Handles for bus infrastructure spawned during agent startup.
struct BusInfrastructure {
    agent_subscriber: crate::bus::Subscriber<crate::bus::MessageEvent>,
    error_subscriber: crate::bus::Subscriber<crate::bus::ErrorEvent>,
    /// See `AgentRuntime::pulse_result_subscriber`'s own doc comment for
    /// why this is a second, independent subscription rather than sharing
    /// one with `background::listener`.
    pulse_result_subscriber: crate::bus::Subscriber<crate::bus::AgentResultEvent>,
    notify_handles: Vec<tokio::task::JoinHandle<()>>,
    bus_infra_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Subscribe to bus topics and spawn bus-level infrastructure tasks.
///
/// Subscribes the agent and error notification channels, spawns notify subscribers,
/// the background result bridge, notification router, and subagent registry.
async fn spawn_bus_infrastructure(
    core: &GatewayCore,
    parts: &mut crate::gateway::startup::GatewayComponents,
) -> Result<BusInfrastructure, FatalError> {
    let agent_subscriber = core
        .bus_handle
        .subscribe(crate::bus::topics::UserMessage)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to subscribe to user:message: {e}")))?;
    let error_subscriber = core
        .bus_handle
        .subscribe(crate::bus::topics::Notification(
            crate::bus::NotifyName::from(crate::bus::SYSTEM_CHANNEL),
        ))
        .await
        .map_err(|e| {
            FatalError::Gateway(format!("failed to subscribe to system notifications: {e}"))
        })?;
    let pulse_result_subscriber = core
        .bus_handle
        .subscribe(crate::bus::topics::Background)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to subscribe to background: {e}")))?;

    let notify_handles = crate::gateway::startup::spawn_notify_subscribers(
        &core.bus_handle,
        &parts.channel_configs,
        &parts.http_client,
        &parts.layout,
        parts.tz,
    )
    .await;

    let mut bus_infra_handles = Vec::new();
    if let Some(h) = crate::notify::router::spawn_notification_router(
        &core.bus_handle,
        parts.endpoint_registry.clone(),
        core.publisher.clone(),
    )
    .await
    {
        bus_infra_handles.push(h);
    }
    if let Some(h) = crate::background::listener::spawn_listener(
        Arc::clone(&parts.spawn_context),
        &core.bus_handle,
    )
    .await
    {
        bus_infra_handles.push(h);
    }

    Ok(BusInfrastructure {
        agent_subscriber,
        error_subscriber,
        pulse_result_subscriber,
        notify_handles,
        bus_infra_handles,
    })
}

/// Assemble the `AgentRuntime` from initialized parts and spawned handles.
async fn build_runtime(
    mut parts: crate::gateway::startup::GatewayComponents,
    core: GatewayCore,
    receivers: CoreReceivers,
    cfg: Config,
    hub_cfg: HubConfig,
    spawned: SpawnedHandles,
    channels: RuntimeChannels,
) -> Result<AgentRuntime, FatalError> {
    let infra = spawn_bus_infrastructure(&core, &mut parts).await?;
    spawned.sessions_ready_tx.send_replace(true);
    let pulse_state_path = parts.layout.pulse_state_json();
    // Both post-turn workers report into the same channel — the main loop
    // applies whichever kind of result arrives, see `run_agent_loop`'s own
    // select branch for it.
    let (post_turn_result_tx, post_turn_result_rx) = tokio::sync::mpsc::unbounded_channel();
    let a2a = spawned.a2a;
    channels
        .a2a_router_tx
        .send_replace(a2a.as_ref().map(|agent| agent.router.clone()));

    let tracing_service = Arc::clone(&channels.services.tracing_service);
    Ok(AgentRuntime {
        name: channels.name,
        services: channels.services,
        layout: parts.layout,
        tz: parts.tz,
        agent: parts.agent,
        observer: Arc::new(parts.observer),
        merge_writer: parts.merge_writer,
        subconscious: parts.subconscious,
        learning_state: Arc::new(std::sync::Mutex::new(
            crate::subconscious::LearningState::default(),
        )),
        post_turn_observe: crate::gateway::post_turn::ObserveWorker::new(
            post_turn_result_tx.clone(),
        ),
        post_turn_subconscious: crate::gateway::post_turn::SubconsciousWorker::new(
            post_turn_result_tx,
        ),
        post_turn_result_rx,
        hybrid_searcher: parts.hybrid_searcher,
        session_runtime: parts.session_runtime,
        session_registry: parts.session_registry,
        agent_messenger: parts.agent_messenger,
        conversation_router: parts.conversation_router,
        action_store: parts.action_store,
        action_notify: parts.action_notify,
        mcp_registry: parts.mcp_registry,
        tools_path: parts.tools_path,
        agent_keys: parts.agent_keys,
        skill_state: parts.skill_state,
        pulse_enabled: parts.pulse_enabled,
        notify_handles: infra.notify_handles,
        channel_configs: parts.channel_configs.clone(),
        webhooks: spawned.webhooks,
        bus_infra_handles: infra.bus_infra_handles,
        http_client: parts.http_client,
        spawn_context: parts.spawn_context,
        model_call_resources_tx: channels.model_call_resources_tx,
        bus_handle: core.bus_handle,
        publisher: core.publisher,
        agent_subscriber: infra.agent_subscriber,
        endpoint_registry: parts.endpoint_registry,
        error_subscriber: infra.error_subscriber,
        pulse_result_subscriber: infra.pulse_result_subscriber,
        last_output_endpoint: None,
        output_topic_override_tx: parts.output_topic_override_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        stop_rx: receivers.stop,
        agent_stop_rx: channels.agent_stop_rx,
        pulse_scheduler: PulseScheduler::with_state_path(&pulse_state_path),
        config_dir: core.config_dir.clone(),
        hub_dir: core.hub_dir.clone(),
        last_user_message_instant: None,
        chat_adapters: spawned.adapters,
        a2a,
        a2a_router_tx: channels.a2a_router_tx,
        reload_done_tx: channels.reload_done_tx,
        a2a_hub: parts.a2a_hub,
        a2a_tracker: parts.a2a_tracker,
        checkpoints: parts.checkpoints,
        config_reload_tracker: parts.config_reload_tracker,
        watcher_handle: spawned.watcher_handle,
        root_config_watcher_handle: spawned.root_config_watcher_handle,
        workbench_watcher_handle: spawned.workbench_watcher_handle,
        change_feed_handle: spawned.change_feed_handle,
        reload_tx: core.reload_tx,
        command_tx: core.command_tx,
        stop_tx: core.stop_tx,
        path_policy: parts.path_policy,
        tracing_service,
        activity: channels.activity,
        cfg,
        hub_cfg,
    })
}

/// Reload MCP servers from `mcp.json`, returning a plain-language outcome
/// note — for the agent's own transcript when this reload was triggered by
/// its own write (see `handle_workspace_reload`).
async fn reload_mcp_servers_note(rt: &mut AgentRuntime) -> String {
    match crate::workspace::config::load_mcp_servers(&rt.layout.mcp_json()) {
        Ok(servers) => {
            let report = rt
                .mcp_registry
                .write()
                .await
                .reconcile_and_connect(&servers)
                .await;
            tracing::info!(
                started = report.started,
                stopped = report.stopped,
                "MCP servers reconciled"
            );
            format!(
                "mcp.json reloaded ({} started, {} stopped)",
                report.started, report.stopped
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload mcp.json, keeping current servers");
            format!("mcp.json failed to reload, keeping current servers: {e}")
        }
    }
}

/// Reload notification channel subscribers from `channels.toml`, returning a
/// plain-language outcome note. Parses the new file before touching
/// anything running: a parse failure must leave the current subscribers in
/// place rather than aborting them and spawning nothing.
async fn reload_channels_note(rt: &mut AgentRuntime) -> String {
    match crate::workspace::config::load_channel_configs(&rt.layout.channels_toml()) {
        Ok(configs) => {
            for h in rt.notify_handles.drain(..) {
                h.abort();
            }
            let new_handles = crate::gateway::startup::spawn_notify_subscribers(
                &rt.bus_handle,
                &configs,
                &rt.http_client,
                &rt.layout,
                rt.tz,
            )
            .await;
            rt.notify_handles = new_handles;
            rt.endpoint_registry.refresh(&rt.cfg, &configs);
            rt.channel_configs = configs;
            "channels.toml reloaded".to_string()
        }
        Err(e) => {
            let message = format!(
                "Couldn't reload your notification channels ({e}). Your existing channels are still running; fix channels.toml and reload again."
            );
            tracing::warn!(error = %e, "failed to reload channels.toml, keeping current channels");
            crate::gateway::helpers::publish_notice(&rt.publisher, message.clone()).await;
            message
        }
    }
}

/// Reload the agent card, if A2A is enabled, returning a failure note (the
/// listener keeps serving the last good card either way, but the operator —
/// and, when this reload was agent-triggered, the agent — still need to
/// know).
async fn reload_agent_card_note(rt: &mut AgentRuntime) -> Option<String> {
    let card_state = &rt.a2a.as_ref()?.card_state;
    let card_runtime =
        crate::a2a::CardRuntime::from_config(&rt.cfg.a2a, &rt.cfg.gateway.bind, &rt.cfg.agent_name);
    let Err(e) = card_state.reload(&rt.layout.agent_card_json(), &card_runtime) else {
        return None;
    };
    let message = format!("agent-card.json failed to reload, still serving the previous card: {e}");
    tracing::warn!(error = %e, "failed to reload agent-card.json, keeping the last good card");
    if let Err(publish_err) = rt
        .publisher
        .publish(
            crate::bus::topics::Notification(crate::bus::NotifyName::from(
                crate::bus::SYSTEM_CHANNEL,
            )),
            crate::bus::NoticeEvent {
                message: message.clone(),
            },
        )
        .await
    {
        tracing::warn!(error = %publish_err, "failed to publish agent card reload notice");
    }
    Some(message)
}

/// Handle a workspace config reload (mcp.json or channels.toml changed).
async fn handle_workspace_reload(rt: &mut AgentRuntime) {
    tracing::info!("handling workspace config reload");

    // Consumed once, up front: whether this specific reload is the one the
    // agent's own `write_file`/`edit_file` call to mcp.json/channels.toml/
    // a2a.json caused — see `ConfigWriteWatch`. Collects a plain-language
    // line per subsystem below (mirroring the wording already logged or
    // published as a user notice) so, if `true`, one summary reaches the
    // agent's own transcript too.
    let deliver_to_agent = rt
        .config_reload_tracker
        .take_if_matches(crate::tools::config_reload_tracker::ConfigReloadKind::Workspace);
    let mut agent_notes: Vec<String> = vec![
        reload_mcp_servers_note(rt).await,
        reload_channels_note(rt).await,
    ];

    // Reload the A2A client's remote agents. Bad entries are skipped with a
    // warning; a read/parse failure keeps the current agents. No outcome is
    // reported back to the agent here: `reload_from_file` only logs, with no
    // success/failure signal this caller can read.
    rt.a2a_hub
        .reload_from_file(&rt.layout.a2a_agents_json(), &rt.agent_keys)
        .await;

    if let Some(note) = reload_agent_card_note(rt).await {
        agent_notes.push(note);
    }

    if deliver_to_agent {
        rt.agent.inject_system_message(format!(
            "workspace configuration reloaded: {}",
            agent_notes.join("; ")
        ));
    }

    if let Err(e) = rt
        .publisher
        .publish(
            crate::bus::topics::Notification(crate::bus::NotifyName::from(
                crate::bus::SYSTEM_CHANNEL,
            )),
            crate::bus::NoticeEvent {
                message: "workspace configuration reloaded".to_string(),
            },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish workspace reload notice");
    }
}

/// Apply a reload signal's idle action to the idle deadline.
async fn apply_idle_action(
    idle_action: reload::IdleAction,
    idle_deadline: &mut Option<tokio::time::Instant>,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match idle_action {
        reload::IdleAction::None => {}
        reload::IdleAction::Disable => {
            *idle_deadline = None;
        }
        reload::IdleAction::Recalculate { new_timeout } => {
            if let Some(last_msg) = rt.last_user_message_instant {
                let new_dl = last_msg + new_timeout;
                if new_dl > tokio::time::Instant::now() {
                    *idle_deadline = Some(new_dl);
                } else {
                    idle::execute_idle_transition(rt, observe_deadline).await;
                    *idle_deadline = None;
                }
            } else {
                *idle_deadline = Some(tokio::time::Instant::now() + new_timeout);
            }
        }
    }
}

/// Wait until a deadline fires, or pend forever if no deadline is set.
async fn wait_for_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// Fork sessions for every due scheduled action.
async fn check_and_run_due_actions(rt: &mut AgentRuntime) {
    actions::spawn_due_actions(&rt.action_store, &rt.publisher).await;
}

/// Maximum time to wait for live sessions to stop and finish recording their
/// runs during graceful shutdown, before giving up and leaving the rest to
/// startup recovery.
const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time to wait for an in-flight post-turn background cycle
/// (observe or subconscious) to finish during graceful shutdown before
/// giving up on it — its own writes are already atomic, so a cycle cut
/// short here loses only whatever it hadn't yet persisted, never leaves
/// something half-written.
const POST_TURN_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// Gracefully shut down the agent: its post-turn cycles, live sessions,
/// adapters, MCP servers, A2A state, and watchers.
///
/// Live sessions are stopped and recorded as interrupted, the same way a
/// process restart records them.
async fn graceful_shutdown(rt: &mut AgentRuntime) {
    tracing::info!(
        notify_handles = rt.notify_handles.len(),
        bus_infra_handles = rt.bus_infra_handles.len(),
        "beginning graceful shutdown"
    );
    // Before sessions: a post-turn cycle may itself be about to publish a
    // notice or spawn a learner, which still needs the bus infrastructure
    // (aborted further down) alive to land.
    rt.post_turn_observe
        .shutdown(POST_TURN_SHUTDOWN_TIMEOUT)
        .await;
    rt.post_turn_subconscious
        .shutdown(POST_TURN_SHUTDOWN_TIMEOUT)
        .await;
    // Stop live sessions first, while the bus and its subscribers (notify
    // router included) are still running, so their results are recorded and
    // delivered rather than left for startup recovery on the next boot.
    rt.session_runtime.shutdown(SESSION_SHUTDOWN_TIMEOUT).await;
    for h in rt.notify_handles.drain(..) {
        h.abort();
    }
    for h in rt.bus_infra_handles.drain(..) {
        h.abort();
    }
    rt.mcp_registry.write().await.disconnect_all().await;
    // Waited for, so a restart of this agent never has two copies of the same
    // bot connected at once.
    rt.chat_adapters.shutdown_all().await;
    rt.a2a = None;
    rt.a2a_router_tx.send_replace(None);
    rt.services.sibling_fanout.unregister(&rt.name).await;
    for handle in [
        rt.watcher_handle.take(),
        rt.root_config_watcher_handle.take(),
        rt.workbench_watcher_handle.take(),
        rt.change_feed_handle.take(),
    ]
    .into_iter()
    .flatten()
    {
        handle.abort();
    }
    rt.activity.run_ended();
    tracing::info!("graceful shutdown complete");
}

/// Apply a finished background post-turn cycle's `Agent`-touching tail — the
/// one thing neither `post_turn` worker can do for itself. See that
/// module's own docs for why this exists as a deferred, drained-by-the-loop
/// step instead of being awaited inline by whichever turn triggered it.
async fn apply_post_turn_result(
    rt: &mut AgentRuntime,
    result: crate::gateway::post_turn::PostTurnResult,
) {
    use crate::gateway::post_turn::PostTurnResult;
    match result {
        PostTurnResult::ObservationReady => {
            crate::gateway::memory::apply_observation_reload(&mut rt.agent, &rt.layout).await;
        }
        PostTurnResult::IdleObservationReady {
            reload_needed,
            continuation,
        } => {
            if reload_needed {
                crate::gateway::memory::apply_observation_reload(&mut rt.agent, &rt.layout).await;
            }
            idle::apply_idle_continuation(rt, &continuation);
        }
        PostTurnResult::SubconsciousNotes(notes) => {
            for note in notes {
                rt.agent
                    .inject_system_message(format!("[Subconscious note] {note}"));
            }
        }
    }
}

/// Run the memory observation pipeline.
fn run_observation(rt: &AgentRuntime) {
    let mem = crate::gateway::memory::MemorySubsystems {
        observer: Arc::clone(&rt.observer),
        merge_writer: Arc::clone(&rt.merge_writer),
        layout: rt.layout.clone(),
        tz: rt.tz,
        publisher: rt.publisher.clone(),
    };
    rt.post_turn_observe.trigger(mem);
}

/// Await a `JoinHandle` if present, or pend forever if `None`.
///
/// On completion, the slot is cleared to prevent re-polling a finished handle.
async fn poll_handle(
    handle: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<(), tokio::task::JoinError> {
    match handle {
        Some(h) => {
            let result = h.await;
            *handle = None;
            result
        }
        None => std::future::pending().await,
    }
}

/// Resolves when the first of the workspace watchers exits, naming which
/// one; pends forever while none are running.
async fn next_log_only_task_exit(
    watcher: &mut Option<tokio::task::JoinHandle<()>>,
    root_config_watcher: &mut Option<tokio::task::JoinHandle<()>>,
    workbench_watcher: &mut Option<tokio::task::JoinHandle<()>>,
    change_feed: &mut Option<tokio::task::JoinHandle<()>>,
) -> (&'static str, Result<(), tokio::task::JoinError>) {
    tokio::select! {
        result = poll_handle(watcher) => ("workspace config watcher", result),
        result = poll_handle(root_config_watcher) => ("root config watcher", result),
        result = poll_handle(workbench_watcher) => ("artifact reload watcher", result),
        result = poll_handle(change_feed) => ("workspace change feed", result),
    }
}

/// Logs an unexpected exit or failure of a background adapter task, and
/// auto-reports it.
///
/// Used for the tasks `run_agent_loop` only logs on exit.
async fn log_adapter_task_exit(
    rt: &AgentRuntime,
    task_name: &str,
    result: &Result<(), tokio::task::JoinError>,
) {
    let described = match result {
        Ok(()) => {
            tracing::error!("{task_name} task exited unexpectedly");
            format!("{task_name} task exited unexpectedly")
        }
        Err(e) => {
            tracing::error!(error = %e, "{task_name} task failed");
            format!("{task_name} task failed: {e}")
        }
    };
    rt.tracing_service
        .on_error(
            &described,
            crate::tracing_service::client_context::gather_for_bug_report(&rt.cfg),
        )
        .await;
}

/// Action from processing a bus event in the event loop.
enum BusEventAction {
    Continue,
    /// The hub's stop request interrupted the turn this event ran.
    Stop,
    /// The inbound message channel closed.
    BusClosed,
}

/// Handle a single typed message event received on the agent subscriber.
async fn handle_bus_event(
    event: Result<Option<crate::bus::MessageEvent>, crate::bus::BusError>,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> BusEventAction {
    match event {
        Ok(Some(message)) => {
            if message.origin.belongs_to_main() {
                if handle_inbound_message(message, rt, observe_deadline, idle_deadline).await {
                    BusEventAction::Stop
                } else {
                    BusEventAction::Continue
                }
            } else {
                // A group chat, a channel, or a non-owner DM: routes to that
                // conversation's session instead of the main agent's turn.
                // Spawned rather than awaited so a slow conversation delivery
                // (a completing target's teardown) never holds up the main
                // event loop.
                let router = Arc::clone(&rt.conversation_router);
                crate::util::spawn_in_span(async move { router.route(message).await });
                BusEventAction::Continue
            }
        }
        Ok(None) => {
            tracing::error!("bus subscriber closed, shutting down");
            BusEventAction::BusClosed
        }
        Err(e) => {
            tracing::warn!(error = %e, "type mismatch on user:message topic");
            BusEventAction::Continue
        }
    }
}

/// Process one bus event and, when it means the agent should stop running,
/// shut down and report which exit the caller should return.
///
/// Kept separate from the `select!` arm in `run_agent_loop` purely to keep
/// that function's line count down.
async fn apply_bus_event(
    event: Result<Option<crate::bus::MessageEvent>, crate::bus::BusError>,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> Option<AgentExit> {
    match handle_bus_event(event, rt, observe_deadline, idle_deadline).await {
        BusEventAction::Continue => None,
        BusEventAction::Stop => {
            graceful_shutdown(rt).await;
            Some(AgentExit::Stopped)
        }
        BusEventAction::BusClosed => {
            graceful_shutdown(rt).await;
            Some(AgentExit::BusClosed)
        }
    }
}

/// Handle a stop request that arrived while the event loop is idle.
///
/// A turn in progress blocks this whole select on the `agent_subscriber.recv()`
/// arm, so a request only reaches this arm when there is genuinely nothing
/// to stop — reply `false` immediately rather than leaving the caller to
/// time out.
fn handle_idle_stop_request(stop_req: Option<crate::gateway::types::StopRequest>) {
    let Some(req) = stop_req else {
        return;
    };
    tracing::debug!(
        requested = ?req.reply_to,
        "stop request received while idle, nothing to stop"
    );
    if let Some(tx) = req.result_tx {
        tx.send(false).ok();
    }
}

/// Collect `first` plus every signal already queued behind it, one entry per
/// kind, hub before agent before workspace (the agent reload resolves against
/// the hub config, so the hub goes first). Duplicates of a kind coalesce
/// because one reload of that kind picks up every edit made so far.
fn drain_pending_reloads(
    first: ReloadSignal,
    rx: &mut crate::gateway::types::ReloadReceiver,
) -> Vec<ReloadSignal> {
    let mut pending = vec![first];
    while let Ok(next) = rx.try_recv() {
        pending.push(next);
    }
    let mut ordered = Vec::with_capacity(3);
    for kind in [
        ReloadSignal::Hub,
        ReloadSignal::Agent,
        ReloadSignal::Workspace,
    ] {
        if pending.contains(&kind) {
            ordered.push(kind);
        }
    }
    ordered
}

/// Apply one config reload signal: an agent config reload (which may put the
/// agent back to idle), a hub config reload, or a workspace file reload.
async fn dispatch_reload(
    signal: ReloadSignal,
    rt: &mut AgentRuntime,
    idle_deadline: &mut Option<tokio::time::Instant>,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match signal {
        ReloadSignal::Agent => {
            let idle_action = reload::handle_root_reload(rt).await;
            apply_idle_action(idle_action, idle_deadline, rt, observe_deadline).await;
        }
        ReloadSignal::Hub => reload::handle_hub_reload(rt).await,
        ReloadSignal::Workspace => handle_workspace_reload(rt).await,
    }
    rt.reload_done_tx.send_modify(|finished| *finished += 1);
}

/// The span every log line and task of the agent `name` runs inside, so each
/// carries an `agent` field. `residuum logs --agent <name>` filters on it.
pub(crate) fn agent_span(name: &str) -> tracing::Span {
    tracing::info_span!("agent", agent = %name)
}

/// Run the agent's event loop as its own task, inside the agent's span.
///
/// The task's `JoinHandle` resolves with the loop's exit, or with a
/// `JoinError` if the loop panicked; a panic here never unwinds into the hub
/// or another agent.
pub(crate) fn spawn_agent_loop(runtime: AgentRuntime) -> tokio::task::JoinHandle<AgentExit> {
    let span = agent_span(&runtime.name);
    crate::util::spawn_in_span(
        async move {
            // `run_agent_loop`'s state (the accumulated select! branches'
            // locals) has grown past clippy's large-future threshold; boxing
            // moves it to the heap so this call site's own stack frame stays
            // small.
            Box::pin(run_agent_loop(runtime)).await
        }
        .instrument(span),
    )
}

/// Run the agent's main event loop.
///
/// Processes inbound messages, pulse ticks, action ticks, and memory pipeline
/// signals until the hub asks the agent to stop or its inbound channel
/// closes.
async fn run_agent_loop(mut rt: AgentRuntime) -> AgentExit {
    let mut pulse_tick = tokio::time::interval(Duration::from_mins(1));
    let mut action_tick = tokio::time::interval(Duration::from_secs(30));
    pulse_tick.tick().await; // skip first tick

    let mut observe_deadline: Option<tokio::time::Instant> = None;
    let mut idle_deadline: Option<tokio::time::Instant> = None;

    tracing::info!("agent ready, entering main loop");

    loop {
        tokio::select! {
            _ = rt.agent_stop_rx.recv() => {
                tracing::info!("stop requested, shutting down");
                graceful_shutdown(&mut rt).await;
                return AgentExit::Stopped;
            }

            first = rt.reload_rx.recv() => {
                if let Some(first) = first {
                    for signal in drain_pending_reloads(first, &mut rt.reload_rx) {
                        dispatch_reload(signal, &mut rt, &mut idle_deadline, &mut observe_deadline).await;
                    }
                }
            }

            event = rt.agent_subscriber.recv() => {
                if let Some(exit) = apply_bus_event(event, &mut rt, &mut observe_deadline, &mut idle_deadline).await {
                    return exit;
                }
            }

            error_event = rt.error_subscriber.recv() => {
                if let Ok(Some(event)) = error_event {
                    tracing::debug!(message = %event.message, "received system error event, injecting into agent");
                    rt.agent.inject_system_message(format!("[Bus] {}", event.message));
                }
            }

            _ = pulse_tick.tick(), if rt.pulse_enabled => {
                handle_pulse_tick(&mut rt).await;
            }

            result = rt.pulse_result_subscriber.recv() => handle_pulse_result_event(result, &mut rt),

            _ = action_tick.tick() => check_and_run_due_actions(&mut rt).await,

            () = rt.action_notify.notified() => check_and_run_due_actions(&mut rt).await,

            () = wait_for_deadline(observe_deadline) => {
                observe_deadline = None;
                run_observation(&rt);
            }

            () = wait_for_deadline(idle_deadline) => {
                idle::execute_idle_transition(&mut rt, &mut observe_deadline).await;
                idle_deadline = None;
            }

            // A finished background post-turn cycle's `Agent` mutation
            // (see `crate::gateway::post_turn`) — never awaited inline by
            // the turn that triggered it.
            result = rt.post_turn_result_rx.recv() => {
                if let Some(result) = result { apply_post_turn_result(&mut rt, result).await; }
            }

            cmd = rt.command_rx.recv() => {
                if let Some(cmd) = cmd {
                    handle_server_command(cmd, &mut rt, &mut observe_deadline).await;
                }
            }

            // Reached only between turns — a turn in progress blocks this
            // whole select on the `agent_subscriber.recv()` arm above, so a
            // stop request only lands here when there is genuinely nothing
            // to stop.
            stop_req = rt.stop_rx.recv() => {
                handle_idle_stop_request(stop_req);
            }

            (task_name, result) = rt.chat_adapters.next_exit() => {
                log_adapter_task_exit(&rt, task_name, &result).await;
            }

            (task_name, result) = next_log_only_task_exit(
                &mut rt.watcher_handle,
                &mut rt.root_config_watcher_handle,
                &mut rt.workbench_watcher_handle,
                &mut rt.change_feed_handle,
            ) => {
                log_adapter_task_exit(&rt, task_name, &result).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{agent_last_known_good_fallback, drain_pending_reloads};
    use crate::config::HubConfig;
    use crate::gateway::types::{GatewayCore, ReloadSignal};
    use crate::hub::services::HubServices;
    use crate::util::FatalError;

    fn hub_config(dir: &std::path::Path) -> HubConfig {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        HubConfig::load_at(dir).unwrap()
    }

    #[tokio::test]
    async fn fallback_with_no_saved_copy_returns_the_original_error_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("scout");
        let hub = hub_config(&dir.path().join("hub"));
        let services = HubServices::for_tests(dir.path(), &hub).await;
        let (core, _receivers) = GatewayCore::new(agent_dir.join("config"), dir.path().join("hub"));
        let original = FatalError::Config("original problem".to_string());

        let result =
            agent_last_known_good_fallback(&agent_dir, hub, &core.publisher, &services, original)
                .await;

        match result {
            Err(FatalError::Config(msg)) => assert_eq!(msg, "original problem"),
            Err(other) => panic!("expected FatalError::Config, got: {other}"),
            Ok(_) => panic!("expected an error, got Ok"),
        }
    }

    #[tokio::test]
    async fn fallback_with_an_unloadable_saved_copy_returns_the_original_error() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("scout");
        let agent_config_dir = agent_dir.join("config");
        std::fs::create_dir_all(&agent_config_dir).unwrap();
        // A last-known-good pair that exists but is itself broken (should
        // never happen in practice, since it's only ever saved after a
        // successful start) still must not surface its own error in place
        // of the live config's -- the live config is what the user needs to
        // fix.
        std::fs::create_dir_all(dir.path().join("hub")).unwrap();
        std::fs::write(
            dir.path()
                .join("hub")
                .join("scout.config.last-known-good.toml"),
            "not valid toml [[[",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("hub")
                .join("scout.providers.last-known-good.toml"),
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let hub = hub_config(&dir.path().join("hub"));
        let services = HubServices::for_tests(dir.path(), &hub).await;
        let (core, _receivers) = GatewayCore::new(agent_config_dir, dir.path().join("hub"));
        let original = FatalError::Config("original problem".to_string());

        let result =
            agent_last_known_good_fallback(&agent_dir, hub, &core.publisher, &services, original)
                .await;

        match result {
            Err(FatalError::Config(msg)) => assert_eq!(msg, "original problem"),
            Err(other) => panic!("expected FatalError::Config, got: {other}"),
            Ok(_) => panic!("expected an error, got Ok"),
        }
    }

    #[tokio::test]
    async fn back_to_back_hub_and_agent_signals_are_both_processed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ReloadSignal>();

        tx.send(ReloadSignal::Agent).unwrap();
        tx.send(ReloadSignal::Hub).unwrap();

        let first = rx.recv().await.unwrap();
        let processed = drain_pending_reloads(first, &mut rx);
        assert_eq!(
            processed,
            vec![ReloadSignal::Hub, ReloadSignal::Agent],
            "both signals must be processed, hub first"
        );
        assert!(rx.try_recv().is_err(), "queue should be empty afterwards");
    }

    #[tokio::test]
    async fn repeated_signals_of_one_kind_coalesce() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ReloadSignal>();
        for _ in 0..3 {
            tx.send(ReloadSignal::Agent).unwrap();
        }
        let first = rx.recv().await.unwrap();
        assert_eq!(
            drain_pending_reloads(first, &mut rx),
            vec![ReloadSignal::Agent]
        );
    }
}
