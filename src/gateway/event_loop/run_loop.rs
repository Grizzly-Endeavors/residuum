//! Gateway entry point and main event loop.
//!
//! Contains `run_gateway` (initialization + wiring) and `run_event_loop`
//! (the `tokio::select` loop that processes all gateway events).

use std::sync::Arc;

use tokio::time::Duration;

use crate::config::Config;
use crate::gateway::types::{GatewayCore, GatewayExit, GatewayRuntime, GatewayState, ReloadSignal};
use crate::memory::types::Visibility;
use crate::pulse::scheduler::PulseScheduler;
use crate::util::FatalError;

use super::commands::handle_server_command;
use super::http::{AdapterSenders, build_gateway_app, spawn_adapters, spawn_http_server};
use super::pulse::handle_pulse_tick;
use super::turns::{handle_inbound_message, persist_and_maybe_observe};

use crate::gateway::memory::execute_observation;
use crate::gateway::{actions, idle, reload, watcher, web};

/// Start the WebSocket gateway server and run the main event loop.
///
/// Initializes all subsystems, spawns the axum WebSocket server, then enters
/// the event loop via `run_event_loop`.
///
/// # Errors
///
/// Returns `FatalError` if initialization fails or the server cannot bind.
#[tracing::instrument(skip_all, fields(bind = %cfg.gateway.addr()))]
pub async fn run_gateway(cfg: Config) -> Result<GatewayExit, FatalError> {
    reload::backup_config(&cfg.config_dir);

    let (core, receivers) = GatewayCore::new(cfg.config_dir.clone());
    let parts = crate::gateway::startup::initialize(&cfg, &core.publisher).await?;

    let update_status = crate::update::SharedUpdateStatus::default();
    let (restart_tx, restart_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (gateway_shutdown_tx, gateway_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let spawned = spawn_server_and_adapters(
        &core,
        &parts,
        &cfg,
        &update_status,
        &restart_tx,
        &gateway_shutdown_tx,
    )
    .await?;

    let update = UpdateChannels {
        status: update_status,
        restart_tx,
        restart_rx,
        gateway_shutdown_tx,
        gateway_shutdown_rx,
    };
    let cloud_config = cfg.cloud.clone();
    let rt = build_runtime(parts, core, receivers, cfg, spawned, update, cloud_config).await?;

    Ok(run_event_loop(rt).await)
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
}

/// Spawn the HTTP server, chat adapters, cloud tunnel, and workspace watcher.
async fn spawn_server_and_adapters(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    cfg: &Config,
    update_status: &crate::update::SharedUpdateStatus,
    restart_tx: &tokio::sync::mpsc::Sender<()>,
    gateway_shutdown_tx: &tokio::sync::mpsc::Sender<()>,
) -> Result<SpawnedHandles, FatalError> {
    let discord_senders = AdapterSenders {
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
        reload: core.reload_tx.clone(),
        command: core.command_tx.clone(),
    };
    let telegram_senders = discord_senders.clone();
    let (tunnel_status_tx, tunnel_status_rx) =
        tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);
    let tunnel_status_tx = Arc::new(tunnel_status_tx);

    let file_registry = crate::gateway::file_server::FileRegistry::new();
    file_registry.spawn_cleanup_task();
    let state = GatewayState {
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx.clone(),
        inbox_dir: parts.layout.inbox_dir(),
        tz: parts.tz,
        tunnel_status_rx: tunnel_status_rx.clone(),
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
        file_registry: file_registry.clone(),
    };
    let config_api_state = web::ConfigApiState {
        config_dir: cfg.config_dir.clone(),
        workspace_dir: parts.layout.root().to_path_buf(),
        memory_dir: Some(parts.layout.memory_dir()),
        reload_tx: Some(core.reload_tx.clone()),
        setup_done: None,
        secret_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let update_api_state = web::update::UpdateApiState {
        update_status: Arc::clone(update_status),
        restart_tx: restart_tx.clone(),
        gateway_shutdown_tx: gateway_shutdown_tx.clone(),
    };
    let tracing_service = Arc::clone(&parts.tracing_service);
    let tracing_api_state = web::tracing_api::TracingApiState {
        service: Arc::clone(&tracing_service),
        client_context: Arc::clone(&parts.tracing_client_context),
    };
    let app = build_gateway_app(
        state,
        cfg,
        config_api_state,
        update_api_state,
        tracing_api_state,
    );
    let server_handle = spawn_http_server(cfg, app, &core.http_shutdown_tx).await?;
    let adapters = spawn_adapters(cfg, discord_senders, telegram_senders, parts.tz);
    let (tunnel_handle, tunnel_shutdown_tx) = spawn_tunnel(cfg, Arc::clone(&tunnel_status_tx));
    let sigterm = crate::gateway::types::TermSignal::new()
        .map_err(|e| FatalError::Gateway(format!("failed to register termination handler: {e}")))?;
    let _watcher_handle = watcher::spawn_workspace_watcher(
        parts.layout.mcp_json(),
        parts.layout.channels_toml(),
        core.reload_tx.clone(),
    );

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
    })
}

/// Update and lifecycle channels bundled to reduce argument count.
struct UpdateChannels {
    status: crate::update::SharedUpdateStatus,
    restart_tx: tokio::sync::mpsc::Sender<()>,
    restart_rx: tokio::sync::mpsc::Receiver<()>,
    gateway_shutdown_tx: tokio::sync::mpsc::Sender<()>,
    gateway_shutdown_rx: tokio::sync::mpsc::Receiver<()>,
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

    let background_result_rx = parts
        .background_result_rx
        .take()
        .ok_or_else(|| FatalError::Gateway("background_result_rx already consumed".to_string()))?;

    let mut bus_infra_handles = Vec::new();
    let shared_result_rx = Arc::new(tokio::sync::Mutex::new(background_result_rx));
    bus_infra_handles.push(crate::background::bridge::spawn_result_bridge(
        shared_result_rx,
        core.publisher.clone(),
        parts.tz,
    ));
    if let Some(h) = crate::notify::router::spawn_notification_router(
        &core.bus_handle,
        &parts.spawn_context,
        parts.endpoint_registry.clone(),
        core.publisher.clone(),
        parts.layout.alerts_md(),
    )
    .await
    {
        bus_infra_handles.push(h);
    }
    let registry = crate::subagents::SubagentRegistry::new(
        Arc::clone(&parts.background_spawner),
        Arc::clone(&parts.spawn_context),
        Arc::clone(&parts.project_state),
        Arc::clone(&parts.skill_state),
        Arc::clone(&parts.mcp_registry),
        parts.layout.subagents_dir(),
    );
    if let Some(h) = crate::subagents::registry::spawn_registry(registry, &core.bus_handle).await {
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
    update: UpdateChannels,
    cloud_config: Option<crate::config::CloudConfig>,
) -> Result<GatewayRuntime, FatalError> {
    let infra = spawn_bus_infrastructure(&core, &mut parts).await?;

    Ok(GatewayRuntime {
        layout: parts.layout,
        tz: parts.tz,
        agent: parts.agent,
        observer: parts.observer,
        reflector: parts.reflector,
        search_index: parts.search_index,
        vector_store: parts.vector_store,
        embedding_provider: parts.embedding_provider,
        hybrid_searcher: parts.hybrid_searcher,
        background_spawner: parts.background_spawner,
        action_store: parts.action_store,
        action_notify: parts.action_notify,
        mcp_registry: parts.mcp_registry,
        project_state: parts.project_state,
        skill_state: parts.skill_state,
        pulse_enabled: parts.pulse_enabled,
        notify_handles: infra.notify_handles,
        bus_infra_handles: infra.bus_infra_handles,
        http_client: parts.http_client,
        spawn_context: parts.spawn_context,
        bus_handle: core.bus_handle,
        publisher: core.publisher,
        agent_subscriber: infra.agent_subscriber,
        endpoint_registry: parts.endpoint_registry,
        error_subscriber: infra.error_subscriber,
        last_output_endpoint: None,
        output_topic_override_tx: parts.output_topic_override_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        server_handle: spawned.server_handle,
        pulse_scheduler: PulseScheduler::new(),
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
        reload_tx: core.reload_tx,
        command_tx: core.command_tx,
        file_registry: spawned.file_registry,
        path_policy: parts.path_policy,
        tracing_service: spawned.tracing_service,
        update_status: update.status,
        restart_tx: update.restart_tx,
        restart_rx: update.restart_rx,
        gateway_shutdown_tx: update.gateway_shutdown_tx,
        gateway_shutdown_rx: update.gateway_shutdown_rx,
        cfg,
    })
}

/// Spawn the cloud tunnel task if configured, returning its handle and shutdown sender.
fn spawn_tunnel(
    cfg: &Config,
    status_tx: Arc<tokio::sync::watch::Sender<crate::tunnel::TunnelStatus>>,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::sync::watch::Sender<bool>>,
) {
    if let Some(ref cloud_cfg) = cfg.cloud {
        let cloud = cloud_cfg.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(cloud, shutdown_rx, status_tx).await;
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

    // Reload notification channel subscribers
    // Abort old subscriber handles
    for h in rt.notify_handles.drain(..) {
        h.abort();
    }
    match crate::workspace::config::load_channel_configs(&rt.layout.channels_toml()) {
        Ok(configs) => {
            let new_handles = crate::gateway::startup::spawn_notify_subscribers(
                &rt.bus_handle,
                &configs,
                &rt.http_client,
                &rt.layout,
                rt.tz,
            )
            .await;
            rt.notify_handles = new_handles;
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload channels.toml, keeping current channels");
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

/// Spawn due actions and handle any resulting main turns.
async fn check_and_run_due_actions(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    let main_turns = actions::spawn_due_actions(&rt.action_store, &rt.publisher).await;
    handle_action_main_turns(main_turns, rt, observe_deadline).await;
}

/// Inject main-turn prompts from scheduled actions and run a wake turn.
async fn handle_action_main_turns(
    main_turns: Vec<actions::ActionMainTurn>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    if main_turns.is_empty() {
        return;
    }

    tracing::info!(
        count = main_turns.len(),
        "running scheduled action main turns"
    );
    for turn in &main_turns {
        tracing::debug!(action = %turn.action_name, "injecting action main turn");
        let formatted = format!("[Scheduled action: {}]\n{}", turn.action_name, turn.prompt);
        rt.agent.inject_system_message(formatted.clone());
        let msgs = [crate::models::Message::system(&formatted)];
        persist_and_maybe_observe(rt, &msgs, Visibility::Background, observe_deadline).await;
    }
}

/// Gracefully shut down all adapters, MCP servers, and the HTTP server.
async fn graceful_shutdown(rt: &mut GatewayRuntime) {
    tracing::info!(
        notify_handles = rt.notify_handles.len(),
        bus_infra_handles = rt.bus_infra_handles.len(),
        "beginning graceful shutdown"
    );
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
        reflector: &rt.reflector,
        search_index: &rt.search_index,
        layout: &rt.layout,
        vector_store: rt.vector_store.as_ref(),
        embedding_provider: rt.embedding_provider.as_ref(),
    };
    execute_observation(&mem, &mut rt.agent).await;
}

/// Respawn the cloud tunnel after an unexpected exit.
fn respawn_tunnel(rt: &mut GatewayRuntime) {
    if let Some(ref cloud_cfg) = rt.cloud_config {
        let cloud = cloud_cfg.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let status_tx = Arc::clone(&rt.tunnel_status_tx);
        status_tx
            .send(crate::tunnel::TunnelStatus::Disconnected)
            .ok();
        rt.tunnel_handle = Some(crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(cloud, shutdown_rx, status_tx).await;
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

/// Action from processing a bus event in the event loop.
enum BusEventAction {
    Continue,
    Exit(GatewayExit),
    Shutdown,
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
            };
            if let Some(exit) =
                handle_inbound_message(message, rt, observe_deadline, idle_deadline).await
            {
                return BusEventAction::Exit(exit);
            }
            BusEventAction::Continue
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

/// Run the main gateway event loop.
///
/// Processes inbound messages, pulse ticks, action ticks, and memory pipeline
/// signals until shutdown or reload is requested.
#[expect(
    clippy::too_many_lines,
    reason = "select! loop with many event sources"
)]
async fn run_event_loop(mut rt: GatewayRuntime) -> GatewayExit {
    let mut pulse_tick = tokio::time::interval(Duration::from_secs(60));
    let mut action_tick = tokio::time::interval(Duration::from_secs(30));
    let mut update_check_tick = tokio::time::interval(Duration::from_secs(6 * 60 * 60));
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
                match handle_bus_event(event, &mut rt, &mut observe_deadline, &mut idle_deadline).await {
                    BusEventAction::Continue => {}
                    BusEventAction::Exit(exit) => return exit,
                    BusEventAction::Shutdown => { graceful_shutdown(&mut rt).await; break; }
                }
            }

            error_event = rt.error_subscriber.recv() => {
                if let Ok(Some(event)) = error_event {
                    tracing::debug!(message = %event.message, "received system error event, injecting into agent");
                    rt.agent.inject_system_message(format!("[Bus] {}", event.message));
                }
            }

            _ = pulse_tick.tick(), if rt.pulse_enabled => {
                handle_pulse_tick(&mut rt, &mut observe_deadline).await;
            }

            _ = action_tick.tick() => {
                check_and_run_due_actions(&mut rt, &mut observe_deadline).await;
            }

            () = rt.action_notify.notified() => {
                check_and_run_due_actions(&mut rt, &mut observe_deadline).await;
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
                match &result {
                    Ok(()) => tracing::error!("tunnel task exited unexpectedly, attempting respawn"),
                    Err(e) => tracing::error!(error = %e, "tunnel task failed, attempting respawn"),
                }
                respawn_tunnel(&mut rt);
            }

            result = poll_handle(&mut rt.discord_handle) => {
                match &result {
                    Ok(()) => tracing::error!("discord adapter task exited unexpectedly"),
                    Err(e) => tracing::error!(error = %e, "discord adapter task failed"),
                }
            }

            result = poll_handle(&mut rt.telegram_handle) => {
                match &result {
                    Ok(()) => tracing::error!("telegram adapter task exited unexpectedly"),
                    Err(e) => tracing::error!(error = %e, "telegram adapter task failed"),
                }
            }
        }
    }

    GatewayExit::Shutdown
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use crate::gateway::types::ReloadSignal;

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
