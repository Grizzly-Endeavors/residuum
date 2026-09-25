//! Gateway entry point and main event loop.
//!
//! Contains `run_gateway` (initialization + wiring) and `run_event_loop`
//! (the `tokio::select` loop that processes all gateway events).

use std::sync::Arc;

use tokio::time::Duration;

use crate::config::Config;
use crate::gateway::types::{
    CoreReceivers, GatewayCore, GatewayExit, GatewayRuntime, GatewayState, ReloadSignal,
};
use crate::pulse::scheduler::PulseScheduler;
use crate::util::FatalError;

use super::commands::handle_server_command;
use super::http::{
    A2aListenerDeps, AdapterSenders, build_gateway_app, spawn_adapters, spawn_http_server,
};
use super::pulse::handle_pulse_tick;
use super::turns::handle_inbound_message;

use crate::gateway::memory::execute_observation;
use crate::gateway::{actions, idle, last_known_good, reload, watcher, web};

/// Start the WebSocket gateway server and run the main event loop.
///
/// Loads config from `config_dir`. If the live files fail to load, or load
/// but the gateway can't actually start on them (no usable main provider,
/// etc.), falls back to the last-known-good copy (see [`last_known_good`])
/// instead of refusing to start. A fallback is announced with a notice once
/// the gateway is up, so it's visible to anyone using the web UI.
///
/// # Errors
///
/// Returns `FatalError` if the config can't be loaded and no working
/// last-known-good copy is available, or the server cannot bind.
#[tracing::instrument(skip_all, fields(config_dir = %config_dir.display()))]
pub async fn run_gateway(config_dir: &std::path::Path) -> Result<GatewayExit, FatalError> {
    let (core, receivers) = GatewayCore::new(config_dir.to_path_buf());
    let (cfg, parts, fallback_problem) = load_and_initialize(config_dir, &core.publisher).await?;
    Box::pin(run_gateway_from_parts(
        cfg,
        parts,
        core,
        receivers,
        fallback_problem,
    ))
    .await
}

/// Start the gateway from an already-built `Config`, bypassing the
/// last-known-good load-and-fallback in [`run_gateway`].
///
/// Used by the setup wizard's isolated temp-directory flow, which builds
/// its own `Config` (with an overridden workspace directory) instead of
/// loading one from a config directory — there's no last-known-good copy
/// to fall back to there anyway.
///
/// # Errors
///
/// Returns `FatalError` if initialization fails or the server cannot bind.
#[tracing::instrument(skip_all, fields(bind = %cfg.gateway.addr()))]
pub async fn run_gateway_with_config(cfg: Config) -> Result<GatewayExit, FatalError> {
    let (core, receivers) = GatewayCore::new(cfg.config_dir.clone());
    let parts = crate::gateway::startup::initialize(&cfg, &core.publisher).await?;
    Box::pin(run_gateway_from_parts(cfg, parts, core, receivers, None)).await
}

/// Load `config_dir`'s config and initialize the gateway on it, falling
/// back to the last-known-good copy if either step fails. Returns the
/// config actually used, its initialized components, and — only when a
/// fallback was used — a description of what was wrong with the live
/// files, for the caller to publish once the gateway is up.
async fn load_and_initialize(
    config_dir: &std::path::Path,
    publisher: &crate::bus::Publisher,
) -> Result<
    (
        Config,
        crate::gateway::startup::GatewayComponents,
        Option<String>,
    ),
    FatalError,
> {
    match Config::load_at(config_dir) {
        Ok(cfg) => match crate::gateway::startup::initialize(&cfg, publisher).await {
            Ok(parts) => Ok((cfg, parts, None)),
            Err(err) => last_known_good_fallback(config_dir, publisher, err).await,
        },
        Err(err) => last_known_good_fallback(config_dir, publisher, err).await,
    }
}

/// Try starting from the last-known-good config after `original_err` broke
/// the live files. Returns `original_err` untouched if there's no
/// last-known-good copy, or it fails to initialize too — the live files
/// are still the more relevant problem to report in that case.
async fn last_known_good_fallback(
    config_dir: &std::path::Path,
    publisher: &crate::bus::Publisher,
    original_err: FatalError,
) -> Result<
    (
        Config,
        crate::gateway::startup::GatewayComponents,
        Option<String>,
    ),
    FatalError,
> {
    let Ok(lkg_cfg) = last_known_good::load(config_dir) else {
        return Err(original_err);
    };
    match crate::gateway::startup::initialize(&lkg_cfg, publisher).await {
        Ok(parts) => Ok((lkg_cfg, parts, Some(original_err.to_string()))),
        Err(_lkg_init_err) => Err(original_err),
    }
}

/// Shared tail of [`run_gateway`] and [`run_gateway_with_config`]: spawn the
/// server and adapters, report the fallback (or save a fresh last-known-good
/// copy), and enter the event loop.
async fn run_gateway_from_parts(
    cfg: Config,
    parts: crate::gateway::startup::GatewayComponents,
    core: GatewayCore,
    receivers: CoreReceivers,
    fallback_problem: Option<String>,
) -> Result<GatewayExit, FatalError> {
    let update_status = crate::update::SharedUpdateStatus::default();
    let (restart_tx, restart_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (gateway_shutdown_tx, gateway_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (model_call_resources_tx, model_call_resources_rx) = tokio::sync::watch::channel(Arc::new(
        web::model::ModelCallResources::from_spawn_context(&parts.spawn_context),
    ));

    let spawned = spawn_server_and_adapters(
        &core,
        &parts,
        &cfg,
        &update_status,
        &restart_tx,
        &gateway_shutdown_tx,
        model_call_resources_rx,
    )
    .await?;

    // The HTTP listener above is bound only after `startup::initialize`
    // finished, so reaching this point means providers, workspace, and the
    // gateway's own listener are all ready. This marker is what the CLI
    // (`serve`'s startup report) and the update-rollback watchdog both wait
    // on to know the gateway is actually healthy rather than merely running.
    crate::daemon::write_ready_file(&cfg.config_dir);

    if let Some(problem) = fallback_problem {
        tracing::error!(
            error = %problem,
            "startup fell back to the last-known-good config"
        );
        crate::gateway::helpers::publish_notice(
            &core.publisher,
            format!(
                "residuum couldn't start using your current config.toml/providers.toml ({problem}). It's running on the last configuration that worked instead — fix the files above, then reload (or restart residuum) to apply your changes."
            ),
        )
        .await;
    } else {
        last_known_good::save(&cfg.config_dir);
    }

    let channels = RuntimeChannels {
        status: update_status,
        restart_tx,
        restart_rx,
        gateway_shutdown_tx,
        gateway_shutdown_rx,
        model_call_resources_tx,
    };
    let cloud_config = cfg.cloud.clone();
    let rt = build_runtime(parts, core, receivers, cfg, spawned, channels, cloud_config).await?;

    // `run_event_loop`'s state (the accumulated select! branches' locals)
    // has grown past clippy's large-future threshold; boxing moves it to the
    // heap so this call site's own stack frame stays small.
    Ok(Box::pin(run_event_loop(rt)).await)
}

/// Handles returned from spawning the HTTP server, adapters, tunnel, and watcher.
struct SpawnedHandles {
    server_handle: tokio::task::JoinHandle<()>,
    adapters: super::http::AdapterHandles,
    tunnel_handle: Option<tokio::task::JoinHandle<()>>,
    tunnel_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    tunnel_status_tx: Arc<tokio::sync::watch::Sender<crate::tunnel::TunnelStatus>>,
    tunnel_status_rx: tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
    tracing_service: Arc<crate::tracing_service::TracingService>,
    sigterm: crate::gateway::types::TermSignal,
    file_registry: crate::gateway::file_server::FileRegistry,
    webhooks: crate::interfaces::webhook::WebhookTable,
    watcher_handle: Option<tokio::task::JoinHandle<()>>,
    workbench_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    change_feed_handle: Option<tokio::task::JoinHandle<()>>,
    workspace_watch_health: tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
    workbench_serving: crate::workbench::server::WorkbenchServing,
    workbench_listener_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Raised by [`build_runtime`] once the session spawn listener is up.
    sessions_ready_tx: tokio::sync::watch::Sender<bool>,
}

/// Start the workbench artifacts listener beside the gateway.
async fn start_workbench_listener(
    cfg: &Config,
    workbench_dir: &std::path::Path,
) -> (
    crate::workbench::server::WorkbenchServing,
    Option<tokio::sync::watch::Sender<bool>>,
) {
    // Teams' and A2A's configured ports stay free for them, and so do their
    // defaults, so enabling either later can't collide with the artifacts
    // listener.
    let reserved_ports = [
        cfg.teams
            .as_ref()
            .map_or(crate::config::DEFAULT_TEAMS_PORT, |teams| teams.port),
        crate::config::DEFAULT_TEAMS_PORT,
        cfg.a2a.port,
        crate::config::DEFAULT_A2A_PORT,
    ];
    crate::workbench::server::start(
        &cfg.gateway.bind,
        cfg.gateway.port,
        &reserved_ports,
        workbench_dir.to_path_buf(),
    )
    .await
}

/// Bundle of `*ApiState` values `build_gateway_app` needs, split out of
/// `spawn_server_and_adapters` to keep it under the line-count lint.
struct ApiStates {
    config: web::ConfigApiState,
    update: web::update::UpdateApiState,
    tracing: web::tracing_api::TracingApiState,
    memory: web::memory::MemoryApiState,
    model: web::model::ModelApiState,
}

fn build_api_states(
    cfg: &Config,
    parts: &crate::gateway::startup::GatewayComponents,
    core: &GatewayCore,
    update: web::update::UpdateApiState,
    tracing_service: &Arc<crate::tracing_service::TracingService>,
    model_call_resources_rx: tokio::sync::watch::Receiver<Arc<web::model::ModelCallResources>>,
) -> ApiStates {
    ApiStates {
        config: web::ConfigApiState {
            config_dir: cfg.config_dir.clone(),
            workspace_dir: parts.layout.root().to_path_buf(),
            memory_dir: Some(parts.layout.memory_dir()),
            reload_tx: Some(core.reload_tx.clone()),
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: Arc::clone(&parts.checkpoints),
        },
        update,
        tracing: web::tracing_api::TracingApiState {
            service: Arc::clone(tracing_service),
            client_context: Arc::clone(&parts.tracing_client_context),
            session_registry: Arc::clone(&parts.session_registry),
        },
        memory: web::memory::MemoryApiState {
            hybrid_searcher: Arc::clone(&parts.hybrid_searcher),
        },
        model: web::model::ModelApiState {
            resources: model_call_resources_rx,
        },
    }
}

/// Build the `GatewayState` the HTTP app's routes share, bundling the
/// pieces `spawn_server_and_adapters` has already built or spawned by this
/// point — split out purely to keep that function under the line-count lint.
fn build_gateway_state(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    tunnel_status_rx: &tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
    file_registry: &crate::gateway::file_server::FileRegistry,
    webhooks: &crate::interfaces::webhook::WebhookTable,
    workspace_watch_health: &tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
) -> GatewayState {
    GatewayState {
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx.clone(),
        stop_tx: core.stop_tx.clone(),
        agent_inbox_dir: parts.layout.agent_inbox_dir(),
        tz: parts.tz,
        tunnel_status_rx: tunnel_status_rx.clone(),
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
        file_registry: file_registry.clone(),
        webhooks: webhooks.clone(),
        session_registry: Arc::clone(&parts.session_registry),
        session_store: Arc::clone(&parts.session_store),
        agent_messenger: Arc::clone(&parts.agent_messenger),
        skill_state: Arc::clone(&parts.skill_state),
        workspace_watch_health: workspace_watch_health.clone(),
        action_store: Arc::clone(&parts.action_store),
        layout: parts.layout.clone(),
    }
}

/// The A2A listener's runtime dependencies, plus the sender that marks the
/// session spawner ready (raised by [`build_runtime`] once it is subscribed).
fn a2a_listener_deps(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    tunnel_status_rx: &tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
) -> (A2aListenerDeps, tokio::sync::watch::Sender<bool>) {
    let (sessions_ready_tx, sessions_ready_rx) = tokio::sync::watch::channel(false);
    let deps = A2aListenerDeps {
        session_registry: Arc::clone(&parts.session_registry),
        agent_messenger: Arc::clone(&parts.agent_messenger),
        skill_state: Arc::clone(&parts.skill_state),
        bus_handle: core.bus_handle.clone(),
        tunnel_status_rx: tunnel_status_rx.clone(),
        sessions_ready: sessions_ready_rx,
    };
    (deps, sessions_ready_tx)
}

/// Spawn the HTTP server, chat adapters, cloud tunnel, and workspace watcher.
async fn spawn_server_and_adapters(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    cfg: &Config,
    update_status: &crate::update::SharedUpdateStatus,
    restart_tx: &tokio::sync::mpsc::Sender<()>,
    gateway_shutdown_tx: &tokio::sync::mpsc::Sender<()>,
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
    let (tunnel_status_tx, tunnel_status_rx) =
        tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);
    let tunnel_status_tx = Arc::new(tunnel_status_tx);
    // Both the hub and this receiver survive a config reload (the tunnel is
    // restarted in place, not rebuilt), so one long-lived task spawned here
    // at cold start stays correct across reloads without being respawned.
    crate::a2a::spawn_sibling_discovery(Arc::clone(&parts.a2a_hub), tunnel_status_rx.clone());

    let file_registry = crate::gateway::file_server::FileRegistry::new()
        .with_workspace_root(parts.layout.root().to_path_buf());
    file_registry.spawn_cleanup_task();
    let webhooks = crate::interfaces::webhook::WebhookTable::from_config(&cfg.webhooks);
    let (workbench_watcher_handle, change_feed_handle, workspace_watch_health) =
        spawn_change_feed_tasks(core, &parts.layout).await;
    let state = build_gateway_state(
        core,
        parts,
        &tunnel_status_rx,
        &file_registry,
        &webhooks,
        &workspace_watch_health,
    );
    let tracing_service = Arc::clone(&parts.tracing_service);
    let (workbench_serving, workbench_listener_shutdown_tx) =
        start_workbench_listener(cfg, &parts.layout.workbench_dir()).await;
    let update_api_state = web::update::UpdateApiState {
        update_status: Arc::clone(update_status),
        restart_tx: restart_tx.clone(),
        gateway_shutdown_tx: gateway_shutdown_tx.clone(),
        config_dir: cfg.config_dir.clone(),
    };
    let api_states = build_api_states(
        cfg,
        parts,
        core,
        update_api_state,
        &tracing_service,
        model_call_resources_rx,
    );
    let app = build_gateway_app(
        state,
        api_states.config,
        api_states.update,
        api_states.tracing,
        workbench_serving.clone(),
        super::http::ExtraApiStates {
            memory: api_states.memory,
            model: api_states.model,
            a2a_agents: web::a2a::A2aAgentsStatusState {
                hub: Arc::clone(&parts.a2a_hub),
            },
        },
    );
    let server_handle = spawn_http_server(cfg, app, &core.http_shutdown_tx).await?;
    let (a2a_deps, sessions_ready_tx) = a2a_listener_deps(core, parts, &tunnel_status_rx);
    let adapters = spawn_adapters(cfg, &adapter_senders, parts.tz, a2a_deps).await;
    let (tunnel_handle, tunnel_shutdown_tx) =
        spawn_tunnel(cfg, Arc::clone(&tunnel_status_tx), workbench_serving.port());
    #[cfg(unix)]
    let sigterm = crate::gateway::types::TermSignal::new()
        .map_err(|e| FatalError::Gateway(format!("failed to register termination handler: {e}")))?;
    #[cfg(not(unix))]
    let sigterm = crate::gateway::types::TermSignal::new();
    let watcher_handle = Some(watcher::spawn_workspace_watcher(
        parts.layout.mcp_json(),
        parts.layout.channels_toml(),
        parts.layout.agent_card_json(),
        parts.layout.a2a_agents_json(),
        core.reload_tx.clone(),
    ));

    Ok(SpawnedHandles {
        server_handle,
        adapters,
        tunnel_handle,
        tunnel_shutdown_tx,
        tunnel_status_tx,
        tunnel_status_rx,
        tracing_service,
        sigterm,
        file_registry,
        webhooks,
        watcher_handle,
        workbench_watcher_handle,
        change_feed_handle,
        workspace_watch_health,
        workbench_serving,
        workbench_listener_shutdown_tx,
        sessions_ready_tx,
    })
}

/// Start the workspace change feed and the artifact reload watcher that
/// follows it. Returns their handles (reload watcher first) and the feed's
/// health.
async fn spawn_change_feed_tasks(
    core: &GatewayCore,
    layout: &crate::workspace::layout::WorkspaceLayout,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::task::JoinHandle<()>>,
    tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
) {
    let (health_tx, health) =
        tokio::sync::watch::channel(crate::workspace::watch::WatchHealth::Starting);
    let workbench_watcher = match crate::workbench::watcher::spawn_workbench_watcher(
        layout.workbench_dir(),
        &core.bus_handle,
        core.publisher.clone(),
    )
    .await
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe the artifact reload watcher to the workspace change feed; open artifacts won't reload on their own");
            None
        }
    };
    let change_feed = crate::workspace::watch::spawn_change_feed(
        layout.root().to_path_buf(),
        core.publisher.clone(),
        health_tx,
    );
    (workbench_watcher, Some(change_feed), health)
}

/// Update, lifecycle, and model-call channels bundled to reduce argument
/// count on `build_runtime`.
struct RuntimeChannels {
    status: crate::update::SharedUpdateStatus,
    restart_tx: tokio::sync::mpsc::Sender<()>,
    restart_rx: tokio::sync::mpsc::Receiver<()>,
    gateway_shutdown_tx: tokio::sync::mpsc::Sender<()>,
    gateway_shutdown_rx: tokio::sync::mpsc::Receiver<()>,
    /// Pushed a fresh `ModelCallResources` on every config reload; stored on
    /// `GatewayRuntime` so `crate::gateway::reload` can push to it.
    model_call_resources_tx: tokio::sync::watch::Sender<Arc<web::model::ModelCallResources>>,
}

/// Handles for bus infrastructure spawned during gateway startup.
struct BusInfrastructure {
    agent_subscriber: crate::bus::Subscriber<crate::bus::MessageEvent>,
    error_subscriber: crate::bus::Subscriber<crate::bus::ErrorEvent>,
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
        notify_handles,
        bus_infra_handles,
    })
}

/// Assemble the `GatewayRuntime` from initialized parts and spawned handles.
async fn build_runtime(
    mut parts: crate::gateway::startup::GatewayComponents,
    core: GatewayCore,
    receivers: crate::gateway::types::CoreReceivers,
    cfg: Config,
    spawned: SpawnedHandles,
    channels: RuntimeChannels,
    cloud_config: Option<crate::config::CloudConfig>,
) -> Result<GatewayRuntime, FatalError> {
    let infra = spawn_bus_infrastructure(&core, &mut parts).await?;
    spawned.sessions_ready_tx.send_replace(true);
    let pulse_state_path = parts.layout.pulse_state_json();

    Ok(GatewayRuntime {
        layout: parts.layout,
        tz: parts.tz,
        agent: parts.agent,
        observer: Arc::new(parts.observer),
        merge_writer: parts.merge_writer,
        subconscious: parts.subconscious,
        learning_state: crate::subconscious::LearningState::default(),
        hybrid_searcher: parts.hybrid_searcher,
        session_runtime: parts.session_runtime,
        session_registry: parts.session_registry,
        session_store: parts.session_store,
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
        last_output_endpoint: None,
        output_topic_override_tx: parts.output_topic_override_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        stop_rx: receivers.stop,
        server_handle: spawned.server_handle,
        pulse_scheduler: PulseScheduler::with_state_path(&pulse_state_path),
        sigterm: spawned.sigterm,
        http_shutdown_tx: core.http_shutdown_tx,
        config_dir: core.config_dir.clone(),
        last_user_message_instant: None,
        cloud_config,
        tunnel_handle: spawned.tunnel_handle,
        tunnel_shutdown_tx: spawned.tunnel_shutdown_tx,
        tunnel_status_tx: spawned.tunnel_status_tx,
        tunnel_status_rx: spawned.tunnel_status_rx,
        discord_handle: spawned.adapters.discord_handle,
        telegram_handle: spawned.adapters.telegram_handle,
        discord_shutdown_tx: spawned.adapters.discord_shutdown_tx,
        telegram_shutdown_tx: spawned.adapters.telegram_shutdown_tx,
        teams_handle: spawned.adapters.teams_handle,
        teams_shutdown_tx: spawned.adapters.teams_shutdown_tx,
        a2a_handle: spawned.adapters.a2a_handle,
        a2a_shutdown_tx: spawned.adapters.a2a_shutdown_tx,
        a2a_card_state: spawned.adapters.a2a_card_state,
        a2a_hub: parts.a2a_hub,
        a2a_tracker: parts.a2a_tracker,
        checkpoints: parts.checkpoints,
        a2a_public_url: spawned.adapters.a2a_public_url,
        watcher_handle: spawned.watcher_handle,
        workbench_watcher_handle: spawned.workbench_watcher_handle,
        change_feed_handle: spawned.change_feed_handle,
        workspace_watch_health: spawned.workspace_watch_health,
        workbench_listener_shutdown_tx: spawned.workbench_listener_shutdown_tx,
        workbench_serving: spawned.workbench_serving,
        reload_tx: core.reload_tx,
        command_tx: core.command_tx,
        stop_tx: core.stop_tx,
        file_registry: spawned.file_registry,
        path_policy: parts.path_policy,
        tracing_service: spawned.tracing_service,
        update_status: channels.status,
        restart_tx: channels.restart_tx,
        restart_rx: channels.restart_rx,
        gateway_shutdown_tx: channels.gateway_shutdown_tx,
        gateway_shutdown_rx: channels.gateway_shutdown_rx,
        cfg,
    })
}

/// Spawn the cloud tunnel task if configured, returning its handle and shutdown sender.
fn spawn_tunnel(
    cfg: &Config,
    status_tx: Arc<tokio::sync::watch::Sender<crate::tunnel::TunnelStatus>>,
    workbench_port: Option<u16>,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::sync::watch::Sender<bool>>,
) {
    if let Some(ref cloud_cfg) = cfg.cloud {
        let cloud = cloud_cfg.clone();
        let (a2a_port, a2a) = crate::tunnel::a2a_tunnel_params(&cfg.a2a);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(
                cloud,
                workbench_port,
                a2a_port,
                a2a,
                shutdown_rx,
                status_tx,
            )
            .await;
        });
        (Some(handle), Some(shutdown_tx))
    } else {
        (None, None)
    }
}

/// Handle a workspace config reload (mcp.json or channels.toml changed).
async fn handle_workspace_reload(rt: &mut GatewayRuntime) {
    tracing::info!("handling workspace config reload");

    // Reload MCP servers
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
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload mcp.json, keeping current servers");
        }
    }

    // Reload notification channel subscribers. Parse the new file before
    // touching anything running: a parse failure must leave the current
    // subscribers in place rather than aborting them and spawning nothing.
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
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload channels.toml, keeping current channels");
            crate::gateway::helpers::publish_notice(
                &rt.publisher,
                format!(
                    "Couldn't reload your notification channels ({e}). Your existing channels are still running; fix channels.toml and reload again."
                ),
            )
            .await;
        }
    }

    // Reload the A2A client's remote agents. Bad entries are skipped with a
    // warning; a read/parse failure keeps the current agents.
    rt.a2a_hub
        .reload_from_file(&rt.layout.a2a_agents_json(), &rt.agent_keys)
        .await;

    // Reload the agent card, if A2A is enabled. On failure the listener
    // keeps serving the last good card; the operator still needs to know.
    if let Some(card_state) = &rt.a2a_card_state {
        let tunnel_status = rt.tunnel_status_rx.borrow().clone();
        let card_runtime = crate::a2a::CardRuntime::from_config_and_tunnel(
            &rt.cfg.a2a,
            &rt.cfg.gateway.bind,
            &tunnel_status,
        );
        if let Err(e) = card_state.reload(&rt.layout.agent_card_json(), &card_runtime) {
            tracing::warn!(error = %e, "failed to reload agent-card.json, keeping the last good card");
            if let Err(publish_err) = rt
                .publisher
                .publish(
                    crate::bus::topics::Notification(crate::bus::NotifyName::from(
                        crate::bus::SYSTEM_CHANNEL,
                    )),
                    crate::bus::NoticeEvent {
                        message: format!(
                            "agent-card.json failed to reload, still serving the previous card: {e}"
                        ),
                    },
                )
                .await
            {
                tracing::warn!(error = %publish_err, "failed to publish agent card reload notice");
            }
        }
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
    rt: &mut GatewayRuntime,
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
async fn check_and_run_due_actions(rt: &mut GatewayRuntime) {
    actions::spawn_due_actions(&rt.action_store, &rt.publisher).await;
}

/// Maximum time to wait for live sessions to stop and finish recording their
/// runs during graceful shutdown, before giving up and leaving the rest to
/// startup recovery.
const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Gracefully shut down all adapters, MCP servers, and the HTTP server.
async fn graceful_shutdown(rt: &mut GatewayRuntime) {
    tracing::info!(
        notify_handles = rt.notify_handles.len(),
        bus_infra_handles = rt.bus_infra_handles.len(),
        "beginning graceful shutdown"
    );
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
    if let Some(tx) = rt.tunnel_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(tx) = rt.discord_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(tx) = rt.telegram_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(tx) = rt.teams_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(tx) = rt.a2a_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(h) = rt.watcher_handle.take() {
        h.abort();
    }
    if let Some(h) = rt.workbench_watcher_handle.take() {
        h.abort();
    }
    if let Some(h) = rt.change_feed_handle.take() {
        h.abort();
    }
    if let Some(tx) = rt.workbench_listener_shutdown_tx.take() {
        tx.send(true).ok();
    }
    rt.http_shutdown_tx.send(true).ok();
    tracing::info!("graceful shutdown complete");
}

/// Spawn a fire-and-forget update check task.
fn spawn_update_check(status: &crate::update::SharedUpdateStatus) {
    let status = Arc::clone(status);
    crate::util::spawn_monitored("update-check", async move {
        crate::update::check_for_update(&status).await;
    });
}

/// Run the memory observation pipeline.
async fn run_observation(rt: &mut GatewayRuntime) {
    let mem = crate::gateway::memory::MemorySubsystems {
        observer: &rt.observer,
        merge_writer: &rt.merge_writer,
        layout: &rt.layout,
        tz: rt.tz,
        publisher: &rt.publisher,
    };
    execute_observation(&mem, &mut rt.agent).await;
}

/// Log the cloud tunnel task's unexpected exit and respawn it.
fn respawn_tunnel(rt: &mut GatewayRuntime, exit: &Result<(), tokio::task::JoinError>) {
    match exit {
        Ok(()) => tracing::error!("tunnel task exited unexpectedly, attempting respawn"),
        Err(e) => tracing::error!(error = %e, "tunnel task failed, attempting respawn"),
    }
    if let Some(ref cloud_cfg) = rt.cloud_config {
        let cloud = cloud_cfg.clone();
        let (a2a_port, a2a) = crate::tunnel::a2a_tunnel_params(&rt.cfg.a2a);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let status_tx = Arc::clone(&rt.tunnel_status_tx);
        let workbench_port = rt.workbench_serving.port();
        status_tx
            .send(crate::tunnel::TunnelStatus::Disconnected)
            .ok();
        rt.tunnel_handle = Some(crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(
                cloud,
                workbench_port,
                a2a_port,
                a2a,
                shutdown_rx,
                status_tx,
            )
            .await;
        }));
        rt.tunnel_shutdown_tx = Some(shutdown_tx);
        tracing::info!("tunnel respawned after unexpected exit");
    }
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

/// Resolves when the first of the chat adapters or the workspace watchers
/// exits, naming which one; pends forever while none are running.
async fn next_log_only_task_exit(
    discord: &mut Option<tokio::task::JoinHandle<()>>,
    telegram: &mut Option<tokio::task::JoinHandle<()>>,
    teams: &mut Option<tokio::task::JoinHandle<()>>,
    watcher: &mut Option<tokio::task::JoinHandle<()>>,
    workbench_watcher: &mut Option<tokio::task::JoinHandle<()>>,
    change_feed: &mut Option<tokio::task::JoinHandle<()>>,
) -> (&'static str, Result<(), tokio::task::JoinError>) {
    tokio::select! {
        result = poll_handle(discord) => ("discord adapter", result),
        result = poll_handle(telegram) => ("telegram adapter", result),
        result = poll_handle(teams) => ("teams adapter", result),
        result = poll_handle(watcher) => ("workspace config watcher", result),
        result = poll_handle(workbench_watcher) => ("artifact reload watcher", result),
        result = poll_handle(change_feed) => ("workspace change feed", result),
    }
}

/// Logs an unexpected exit or failure of a background adapter task.
///
/// Used for the tasks `run_event_loop` only logs on exit (unlike
/// `tunnel_handle`, which also respawns).
fn log_adapter_task_exit(task_name: &str, result: &Result<(), tokio::task::JoinError>) {
    match result {
        Ok(()) => tracing::error!("{task_name} task exited unexpectedly"),
        Err(e) => tracing::error!(error = %e, "{task_name} task failed"),
    }
}

/// Action from processing a bus event in the event loop.
enum BusEventAction {
    Continue,
    Shutdown,
    /// A restart trigger interrupted the turn this event ran (see
    /// `ShutdownReason::Restart`) — the caller must return `GatewayExit::Restart`
    /// instead of the plain shutdown path.
    Restart,
}

/// Handle a single typed message event received on the agent subscriber.
async fn handle_bus_event(
    event: Result<Option<crate::bus::MessageEvent>, crate::bus::BusError>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> BusEventAction {
    match event {
        Ok(Some(msg_event)) => {
            let message = crate::interfaces::types::InboundMessage {
                id: msg_event.id,
                content: msg_event.content,
                origin: msg_event.origin,
                timestamp: chrono::Utc::now(),
                images: msg_event.images,
                context: msg_event.context,
            };
            if message.origin.belongs_to_main() {
                match handle_inbound_message(message, rt, observe_deadline, idle_deadline).await {
                    None => BusEventAction::Continue,
                    Some(crate::gateway::types::ShutdownReason::Restart) => BusEventAction::Restart,
                    Some(
                        crate::gateway::types::ShutdownReason::Sigterm
                        | crate::gateway::types::ShutdownReason::GatewayShutdown,
                    ) => BusEventAction::Shutdown,
                }
            } else {
                // A group chat, a channel, or a non-owner DM: routes to that
                // conversation's session instead of the main agent's turn.
                // Spawned rather than awaited so a slow conversation delivery
                // (a completing target's teardown) never holds up the main
                // event loop.
                let router = Arc::clone(&rt.conversation_router);
                tokio::spawn(async move { router.route(message).await });
                BusEventAction::Continue
            }
        }
        Ok(None) => {
            tracing::info!("bus subscriber closed, shutting down");
            BusEventAction::Shutdown
        }
        Err(e) => {
            tracing::warn!(error = %e, "type mismatch on user:message topic");
            BusEventAction::Continue
        }
    }
}

/// Process one bus event and, when it means the gateway should stop
/// running, shut down and report which exit the caller should return.
///
/// Kept separate from the `select!` arm in `run_event_loop` purely to keep
/// that function's line count down.
async fn apply_bus_event(
    event: Result<Option<crate::bus::MessageEvent>, crate::bus::BusError>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    match handle_bus_event(event, rt, observe_deadline, idle_deadline).await {
        BusEventAction::Continue => None,
        BusEventAction::Shutdown => {
            graceful_shutdown(rt).await;
            Some(GatewayExit::Shutdown)
        }
        BusEventAction::Restart => {
            graceful_shutdown(rt).await;
            Some(GatewayExit::Restart)
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

/// Run the main gateway event loop.
///
/// Processes inbound messages, pulse ticks, action ticks, and memory pipeline
/// signals until shutdown or reload is requested.
async fn run_event_loop(mut rt: GatewayRuntime) -> GatewayExit {
    let mut pulse_tick = tokio::time::interval(Duration::from_mins(1));
    let mut action_tick = tokio::time::interval(Duration::from_secs(30));
    let mut update_check_tick = tokio::time::interval(Duration::from_hours(6));
    pulse_tick.tick().await; // skip first tick
    spawn_update_check(&rt.update_status);

    let mut observe_deadline: Option<tokio::time::Instant> = None;
    let mut idle_deadline: Option<tokio::time::Instant> = None;

    tracing::info!("gateway ready, entering main loop");

    loop {
        tokio::select! {
            () = rt.sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
                graceful_shutdown(&mut rt).await;
                break;
            }

            _ = rt.reload_rx.changed() => {
                let signal = rt.reload_rx.borrow_and_update().clone();
                match signal {
                    ReloadSignal::None => {}
                    ReloadSignal::Root => {
                        let idle_action = reload::handle_root_reload(&mut rt).await;
                        apply_idle_action(idle_action, &mut idle_deadline, &mut rt, &mut observe_deadline).await;
                    }
                    ReloadSignal::Workspace => {
                        handle_workspace_reload(&mut rt).await;
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

            _ = action_tick.tick() => {
                check_and_run_due_actions(&mut rt).await;
            }

            () = rt.action_notify.notified() => {
                check_and_run_due_actions(&mut rt).await;
            }

            () = wait_for_deadline(observe_deadline) => {
                observe_deadline = None;
                run_observation(&mut rt).await;
            }

            () = wait_for_deadline(idle_deadline) => {
                idle::execute_idle_transition(&mut rt, &mut observe_deadline).await;
                idle_deadline = None;
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

            _ = update_check_tick.tick() => {
                tracing::debug!("scheduled update check triggered");
                spawn_update_check(&rt.update_status);
            }

            _ = rt.restart_rx.recv() => {
                tracing::info!("restart signal received, shutting down for re-exec");
                graceful_shutdown(&mut rt).await;
                return GatewayExit::Restart;
            }

            _ = rt.gateway_shutdown_rx.recv() => {
                tracing::info!("shutdown signal received via HTTP API");
                graceful_shutdown(&mut rt).await;
                break;
            }

            result = poll_handle(&mut rt.tunnel_handle) => {
                respawn_tunnel(&mut rt, &result);
            }

            (task_name, result) = next_log_only_task_exit(
                &mut rt.discord_handle,
                &mut rt.telegram_handle,
                &mut rt.teams_handle,
                &mut rt.watcher_handle,
                &mut rt.workbench_watcher_handle,
                &mut rt.change_feed_handle,
            ) => {
                log_adapter_task_exit(task_name, &result);
            }
        }
    }

    GatewayExit::Shutdown
}

#[cfg(test)]
mod tests {
    use super::last_known_good_fallback;
    use crate::gateway::types::{GatewayCore, ReloadSignal};
    use crate::util::FatalError;

    #[tokio::test]
    async fn fallback_with_no_saved_copy_returns_the_original_error_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (core, _receivers) = GatewayCore::new(dir.path().to_path_buf());
        let original = FatalError::Config("original problem".to_string());

        let result = last_known_good_fallback(dir.path(), &core.publisher, original).await;

        match result {
            Err(FatalError::Config(msg)) => assert_eq!(msg, "original problem"),
            Err(other) => panic!("expected FatalError::Config, got: {other}"),
            Ok(_) => panic!("expected an error, got Ok"),
        }
    }

    #[tokio::test]
    async fn fallback_with_an_unloadable_saved_copy_returns_the_original_error() {
        let dir = tempfile::tempdir().unwrap();
        // A last-known-good pair that exists but is itself broken (should
        // never happen in practice, since it's only ever saved after a
        // successful start) still must not surface its own error in place
        // of the live config's — the live config is what the user needs to
        // fix.
        std::fs::write(
            dir.path().join("config.last-known-good.toml"),
            "not valid toml [[[",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("providers.last-known-good.toml"),
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let (core, _receivers) = GatewayCore::new(dir.path().to_path_buf());
        let original = FatalError::Config("original problem".to_string());

        let result = last_known_good_fallback(dir.path(), &core.publisher, original).await;

        match result {
            Err(FatalError::Config(msg)) => assert_eq!(msg, "original problem"),
            Err(other) => panic!("expected FatalError::Config, got: {other}"),
            Ok(_) => panic!("expected an error, got Ok"),
        }
    }

    #[tokio::test]
    async fn consecutive_reload_signals_both_received() {
        let (tx, mut rx) = tokio::sync::watch::channel(ReloadSignal::None);

        // First send
        tx.send(ReloadSignal::Root).unwrap();
        rx.changed().await.unwrap();
        let val = rx.borrow_and_update().clone();
        assert_eq!(val, ReloadSignal::Root);

        // Second send of the same value — should still wake the receiver
        tx.send(ReloadSignal::Root).unwrap();
        rx.changed().await.unwrap();
        let val2 = rx.borrow_and_update().clone();
        assert_eq!(
            val2,
            ReloadSignal::Root,
            "second identical send should still be received"
        );
    }
}
