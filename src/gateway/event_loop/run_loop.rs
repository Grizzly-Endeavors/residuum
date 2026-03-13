//! Gateway entry point and main event loop.
//!
//! Contains `run_gateway` (initialization + wiring) and `run_event_loop`
//! (the `tokio::select` loop that processes all gateway events).

use std::sync::Arc;

use tokio::time::Duration;

use crate::config::Config;
use crate::error::ResiduumError;
use crate::gateway::protocol::ServerMessage;
use crate::gateway::types::{GatewayCore, GatewayExit, GatewayRuntime, GatewayState, ReloadSignal};
use crate::memory::types::Visibility;
use crate::pulse::scheduler::PulseScheduler;

use super::background::{BackgroundContext, handle_background_result};
use super::commands::handle_server_command;
use super::http::{AdapterSenders, build_gateway_app, spawn_adapters, spawn_http_server};
use super::pulse::handle_pulse_tick;
use super::turns::{handle_inbound_message, persist_and_maybe_observe, run_wake_turn_handler};

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

    let parts = crate::gateway::startup::initialize(&cfg).await?;
    let (core, receivers) = GatewayCore::new(cfg.config_dir.clone());

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
    bus_handle: crate::bus::BusHandle,
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
        inbound: core.inbound_tx.clone(),
        reload: core.reload_tx.clone(),
        command: core.command_tx.clone(),
    };
    let telegram_senders = AdapterSenders {
        inbound: core.inbound_tx.clone(),
        reload: core.reload_tx.clone(),
        command: core.command_tx.clone(),
    };
    let (tunnel_status_tx, tunnel_status_rx) =
        tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);
    let tunnel_status_tx = Arc::new(tunnel_status_tx);

    let state = GatewayState {
        inbound_tx: core.inbound_tx.clone(),
        broadcast_tx: core.broadcast_tx.clone(),
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
        bus_handle: core.bus_handle.clone(),
    })
}

/// Update-related channels bundled to reduce argument count.
struct UpdateChannels {
    status: crate::update::SharedUpdateStatus,
    restart_tx: tokio::sync::mpsc::Sender<()>,
    restart_rx: tokio::sync::mpsc::Receiver<()>,
}

/// Assemble the `GatewayRuntime` from initialized parts and spawned handles.
async fn build_runtime(
    parts: crate::gateway::startup::GatewayComponents,
    core: GatewayCore,
    receivers: crate::gateway::types::CoreReceivers,
    cfg: Config,
    spawned: SpawnedHandles,
    update: UpdateChannels,
    cloud_config: Option<crate::config::CloudConfig>,
) -> Result<GatewayRuntime, ResiduumError> {
    let agent_subscriber = spawned
        .bus_handle
        .subscribe(crate::bus::TopicId::AgentMain)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to subscribe to agent:main: {e}")))?;

    Ok(GatewayRuntime {
        layout: parts.layout,
        tz: parts.tz,
        agent: parts.agent,
        observer: parts.observer,
        reflector: parts.reflector,
        search_index: parts.search_index,
        vector_store: parts.vector_store,
        embedding_provider: parts.embedding_provider,
        action_store: parts.action_store,
        action_notify: parts.action_notify,
        mcp_registry: parts.mcp_registry,
        project_state: parts.project_state,
        skill_state: parts.skill_state,
        pulse_enabled: parts.pulse_enabled,
        notification_router: parts.notification_router,
        http_client: parts.http_client,
        background_spawner: parts.background_spawner,
        background_result_rx: parts.background_result_rx,
        spawn_context: parts.spawn_context,
        inbound_rx: receivers.inbound,
        bus_handle: spawned.bus_handle,
        publisher: core.publisher,
        agent_subscriber,
        last_output_topic: None,
        broadcast_tx: core.broadcast_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        server_handle: spawned.server_handle,
        pulse_scheduler: PulseScheduler::new(),
        sigterm: spawned.sigterm,
        shutdown_tx: core.shutdown_tx,
        config_dir: core.config_dir.clone(),
        last_reply: None,
        unsolicited_handles: std::collections::HashMap::new(),
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
        inbound_tx: core.inbound_tx,
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

    // Reload notification channels
    match crate::workspace::config::load_channel_configs(&rt.layout.channels_toml()) {
        Ok(configs) => {
            let channels = crate::workspace::config::build_external_channels(
                &configs,
                rt.http_client.client(),
            )
            .await;
            rt.notification_router.reload_channels(channels).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload channels.toml, keeping current channels");
        }
    }

    rt.broadcast_tx
        .send(ServerMessage::Notice {
            message: "workspace configuration reloaded".to_string(),
        })
        .ok();
}

/// Handle a background task result in the event loop: route, observe, and optionally wake.
async fn handle_event_loop_bg_result(
    result: crate::background::types::BackgroundResult,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    let bg_ctx = BackgroundContext {
        router: &rt.notification_router,
        layout: &rt.layout,
        observer: &rt.observer,
        project_state: &rt.project_state,
        tz: rt.tz,
    };
    let bg_outcome =
        handle_background_result(result, &bg_ctx, &mut rt.agent, observe_deadline).await;
    if bg_outcome.force_observe {
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
    if bg_outcome.wake_requested
        && let Some(exit) = run_wake_turn_handler(rt, observe_deadline).await
    {
        return Some(exit);
    }
    None
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
    let main_turns = actions::spawn_due_actions(
        &rt.action_store,
        &rt.layout,
        &rt.spawn_context,
        &rt.project_state,
        &rt.skill_state,
        &rt.mcp_registry,
        &rt.background_spawner,
    )
    .await;
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

    run_wake_turn_handler(rt, observe_deadline).await
}

/// Gracefully shut down all adapters, MCP servers, and the HTTP server.
async fn graceful_shutdown(rt: &mut GatewayRuntime) {
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

            msg = rt.inbound_rx.recv() => {
                let Some(routed) = msg else {
                    tracing::info!("inbound channel closed, shutting down");
                    graceful_shutdown(&mut rt).await;
                    break;
                };
                if let Some(exit) = handle_inbound_message(routed, &mut rt, &mut observe_deadline, &mut idle_deadline).await {
                    return exit;
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

            bg_result = rt.background_result_rx.recv() => {
                if let Some(result) = bg_result
                    && let Some(exit) = handle_event_loop_bg_result(result, &mut rt, &mut observe_deadline).await
                {
                    return exit;
                }
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
