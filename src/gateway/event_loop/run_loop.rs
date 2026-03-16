//! Gateway entry point and main event loop.
//!
//! Contains `run_gateway` (initialization + wiring) and `run_event_loop`
//! (the `tokio::select` loop that processes all gateway events).

use std::sync::Arc;

use tokio::time::Duration;

use crate::config::Config;
use crate::error::ResiduumError;
use crate::gateway::types::{GatewayCore, GatewayExit, GatewayRuntime, GatewayState, ReloadSignal};
use crate::memory::types::Visibility;
use crate::pulse::scheduler::PulseScheduler;

use super::commands::handle_server_command;
use super::http::{AdapterSenders, build_gateway_app, spawn_adapters, spawn_http_server};
use super::pulse::handle_pulse_tick;
use super::turns::{handle_inbound_message, persist_and_maybe_observe};

use crate::gateway::memory::{MemorySubsystems, execute_observation};
use crate::gateway::{actions, idle, reload, watcher, web};

/// Start the WebSocket gateway server and run the main event loop.
///
/// Initializes all subsystems, spawns the axum WebSocket server, then enters
/// the event loop via `run_event_loop`.
///
/// # Errors
///
/// Returns `ResiduumError` if initialization fails or the server cannot bind.
pub async fn run_gateway(cfg: Config) -> Result<GatewayExit, ResiduumError> {
    reload::backup_config(&cfg.config_dir);

    let (core, receivers) = GatewayCore::new(cfg.config_dir.clone());
    let parts = crate::gateway::startup::initialize(&cfg, &core.publisher).await?;

    let update_status = crate::update::new_shared_status();
    let (restart_tx, restart_rx) = tokio::sync::mpsc::channel::<()>(1);

    let spawned =
        spawn_server_and_adapters(&core, &parts, &cfg, &update_status, &restart_tx).await?;

    let update = UpdateChannels {
        status: update_status,
        restart_tx,
        restart_rx,
    };
    let cloud_config = cfg.cloud.clone();
    let rt = build_runtime(parts, core, receivers, cfg, spawned, update, cloud_config).await?;

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
    sigterm: tokio::signal::unix::Signal,
}

/// Spawn the HTTP server, chat adapters, cloud tunnel, and workspace watcher.
async fn spawn_server_and_adapters(
    core: &GatewayCore,
    parts: &crate::gateway::startup::GatewayComponents,
    cfg: &Config,
    update_status: &crate::update::SharedUpdateStatus,
    restart_tx: &tokio::sync::mpsc::Sender<()>,
) -> Result<SpawnedHandles, ResiduumError> {
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

    let state = GatewayState {
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx.clone(),
        inbox_dir: parts.layout.inbox_dir(),
        tz: parts.tz,
        tunnel_status_rx: tunnel_status_rx.clone(),
        publisher: core.publisher.clone(),
        bus_handle: core.bus_handle.clone(),
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
    };
    let app = build_gateway_app(state, cfg, config_api_state, update_api_state);
    let server_handle = spawn_http_server(cfg, app, &core.shutdown_tx).await?;
    let adapters = spawn_adapters(cfg, discord_senders, telegram_senders, parts.tz);
    let (tunnel_handle, tunnel_shutdown_tx) = spawn_tunnel(cfg, Arc::clone(&tunnel_status_tx));
    let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| ResiduumError::Gateway(format!("failed to register SIGTERM handler: {e}")))?;
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
        sigterm,
    })
}

/// Update-related channels bundled to reduce argument count.
struct UpdateChannels {
    status: crate::update::SharedUpdateStatus,
    restart_tx: tokio::sync::mpsc::Sender<()>,
    restart_rx: tokio::sync::mpsc::Receiver<()>,
}

/// Assemble the `GatewayRuntime` from initialized parts and spawned handles.
#[expect(
    clippy::too_many_lines,
    reason = "needs refactor — extract bus infrastructure spawning"
)]
async fn build_runtime(
    parts: crate::gateway::startup::GatewayComponents,
    core: GatewayCore,
    receivers: crate::gateway::types::CoreReceivers,
    cfg: Config,
    spawned: SpawnedHandles,
    update: UpdateChannels,
    cloud_config: Option<crate::config::CloudConfig>,
) -> Result<GatewayRuntime, ResiduumError> {
    let agent_subscriber = core
        .bus_handle
        .subscribe_typed(crate::bus::topics::UserMessage)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to subscribe to user:message: {e}")))?;
    let error_subscriber = core
        .bus_handle
        .subscribe(crate::bus::TopicId::BusErrors)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to subscribe to bus:errors: {e}")))?;

    let notify_handles = crate::gateway::startup::spawn_notify_subscribers(
        &core.bus_handle,
        &parts.channel_configs,
        &parts.http_client,
        &parts.layout,
        parts.tz,
    )
    .await;

    // Spawn bus infrastructure (not restarted on workspace reload)
    let mut bus_infra_handles = Vec::new();
    let shared_result_rx = std::sync::Arc::new(tokio::sync::Mutex::new(parts.background_result_rx));
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
    bus_infra_handles
        .push(crate::subagents::registry::spawn_registry(registry, &core.bus_handle).await);

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
        notify_handles,
        bus_infra_handles,
        http_client: parts.http_client,
        spawn_context: parts.spawn_context,
        bus_handle: core.bus_handle,
        publisher: core.publisher,
        agent_subscriber,
        endpoint_registry: parts.endpoint_registry,
        error_subscriber,
        last_output_topic: None,
        output_topic_override_tx: parts.output_topic_override_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        server_handle: spawned.server_handle,
        pulse_scheduler: PulseScheduler::new(),
        sigterm: spawned.sigterm,
        shutdown_tx: core.shutdown_tx,
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
        path_policy: parts.path_policy,
        update_status: update.status,
        restart_tx: update.restart_tx,
        restart_rx: update.restart_rx,
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
        let handle = crate::spawn::spawn_monitored("tunnel", async move {
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
            crate::bus::TopicId::SystemBroadcast,
            crate::bus::BusEvent::Notice {
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
) -> Option<GatewayExit> {
    let main_turns = actions::spawn_due_actions(&rt.action_store, &rt.publisher).await;
    handle_action_main_turns(main_turns, rt, observe_deadline).await
}

/// Inject main-turn prompts from scheduled actions and run a wake turn.
async fn handle_action_main_turns(
    main_turns: Vec<actions::ActionMainTurn>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    if main_turns.is_empty() {
        return None;
    }

    for turn in &main_turns {
        let formatted = format!("[Scheduled action: {}]\n{}", turn.action_name, turn.prompt);
        rt.agent.inject_system_message(formatted.clone());
        let msgs = [crate::models::Message::system(&formatted)];
        persist_and_maybe_observe(rt, &msgs, Visibility::Background, observe_deadline).await;
    }

    None
}

/// Gracefully shut down all adapters, MCP servers, and the HTTP server.
async fn graceful_shutdown(rt: &mut GatewayRuntime) {
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
    rt.shutdown_tx.send(true).ok();
}

/// Spawn a fire-and-forget update check task.
fn spawn_update_check(status: &crate::update::SharedUpdateStatus) {
    let status = Arc::clone(status);
    crate::spawn::spawn_monitored("update-check", async move {
        crate::update::check_for_update(&status).await;
    });
}

/// Run the memory observation pipeline.
async fn run_observation(rt: &mut GatewayRuntime) {
    let mem = MemorySubsystems {
        observer: &rt.observer,
        reflector: &rt.reflector,
        search_index: &rt.search_index,
        layout: &rt.layout,
        vector_store: rt.vector_store.as_ref(),
        embedding_provider: rt.embedding_provider.as_ref(),
    };
    execute_observation(&mem, &mut rt.agent).await;
}

/// Format the reason a task handle completed.
fn handle_exit_reason(result: &Result<(), tokio::task::JoinError>) -> String {
    match result {
        Ok(()) => "exited unexpectedly".to_string(),
        Err(e) => format!("failed: {e}"),
    }
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
        rt.tunnel_handle = Some(crate::spawn::spawn_monitored("tunnel", async move {
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
            _ = rt.sigterm.recv() => {
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
                if let Some(crate::bus::BusEvent::Error { message, .. }) = error_event {
                    tracing::debug!(message = %message, "bus delivery error");
                    rt.agent.inject_system_message(format!("[Bus] {message}"));
                }
            }

            _ = pulse_tick.tick(), if rt.pulse_enabled => {
                if let Some(exit) = handle_pulse_tick(&mut rt, &mut observe_deadline).await {
                    return exit;
                }
            }

            _ = action_tick.tick() => {
                if let Some(exit) = check_and_run_due_actions(&mut rt, &mut observe_deadline).await {
                    return exit;
                }
            }

            () = rt.action_notify.notified() => {
                if let Some(exit) = check_and_run_due_actions(&mut rt, &mut observe_deadline).await {
                    return exit;
                }
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
                spawn_update_check(&rt.update_status);
            }

            _ = rt.restart_rx.recv() => {
                tracing::info!("restart signal received, shutting down for re-exec");
                graceful_shutdown(&mut rt).await;
                return GatewayExit::Restart;
            }

            result = poll_handle(&mut rt.tunnel_handle) => {
                tracing::error!(reason = handle_exit_reason(&result), "tunnel task ended, attempting respawn");
                respawn_tunnel(&mut rt);
            }

            result = poll_handle(&mut rt.discord_handle) => {
                tracing::error!(reason = handle_exit_reason(&result), "discord adapter task ended");
            }

            result = poll_handle(&mut rt.telegram_handle) => {
                tracing::error!(reason = handle_exit_reason(&result), "telegram adapter task ended");
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
