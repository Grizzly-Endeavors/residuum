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
    let rt_inbound_tx = core.inbound_tx.clone();
    let rt_reload_tx = core.reload_tx.clone();
    let rt_command_tx = core.command_tx.clone();

    let (tunnel_status_tx, tunnel_status_rx) =
        tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);

    let state = GatewayState {
        inbound_tx: core.inbound_tx,
        broadcast_tx: core.broadcast_tx.clone(),
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx,
        inbox_dir: parts.layout.inbox_dir(),
        tz: parts.tz,
        tunnel_status_rx: tunnel_status_rx.clone(),
    };
    let config_api_state = web::ConfigApiState {
        config_dir: cfg.config_dir.clone(),
        workspace_dir: parts.layout.root().to_path_buf(),
        memory_dir: Some(parts.layout.memory_dir()),
        reload_tx: Some(core.reload_tx.clone()),
        setup_done: None,
        secret_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let app = build_gateway_app(state, &cfg, config_api_state);
    let server_handle = spawn_http_server(&cfg, app, &core.shutdown_tx).await?;
    let adapters = spawn_adapters(&cfg, discord_senders, telegram_senders, parts.tz);

    let (tunnel_handle, tunnel_shutdown_tx) = spawn_tunnel(&cfg, tunnel_status_tx);

    let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| ResiduumError::Gateway(format!("failed to register SIGTERM handler: {e}")))?;

    let _watcher_handle = watcher::spawn_workspace_watcher(
        parts.layout.mcp_json(),
        parts.layout.channels_toml(),
        core.reload_tx.clone(),
    );

    let rt = GatewayRuntime {
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
        broadcast_tx: core.broadcast_tx,
        reload_rx: receivers.reload,
        command_rx: receivers.command,
        server_handle,
        pulse_scheduler: PulseScheduler::new(),
        sigterm,
        shutdown_tx: core.shutdown_tx,
        config_dir: core.config_dir.clone(),
        last_reply: None,
        unsolicited_handles: std::collections::HashMap::new(),
        last_user_message_instant: None,
        tunnel_handle,
        tunnel_shutdown_tx,
        tunnel_status_rx,
        discord_handle: adapters.discord_handle,
        telegram_handle: adapters.telegram_handle,
        discord_shutdown_tx: adapters.discord_shutdown_tx,
        telegram_shutdown_tx: adapters.telegram_shutdown_tx,
        inbound_tx: rt_inbound_tx,
        reload_tx: rt_reload_tx,
        command_tx: rt_command_tx,
        path_policy: parts.path_policy,
        cfg,
    };

    Ok(Box::pin(run_event_loop(rt)).await)
}

/// Spawn the cloud tunnel task if configured, returning its handle and shutdown sender.
fn spawn_tunnel(
    cfg: &Config,
    status_tx: tokio::sync::watch::Sender<crate::tunnel::TunnelStatus>,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::sync::watch::Sender<bool>>,
) {
    if let Some(ref cloud_cfg) = cfg.cloud {
        let cloud = cloud_cfg.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            crate::tunnel::start_tunnel(cloud, shutdown_rx, status_tx).await;
        });
        (Some(handle), Some(shutdown_tx))
    } else {
        drop(status_tx);
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

/// Run the main gateway event loop.
///
/// Processes inbound messages, pulse ticks, action ticks, and memory pipeline
/// signals until shutdown or reload is requested.
async fn run_event_loop(mut rt: GatewayRuntime) -> GatewayExit {
    let mut pulse_tick = tokio::time::interval(Duration::from_secs(60));
    let mut action_tick = tokio::time::interval(Duration::from_secs(30));
    pulse_tick.tick().await; // skip first tick
    let mut observe_deadline: Option<tokio::time::Instant> = None;
    let mut idle_deadline: Option<tokio::time::Instant> = None;

    tracing::info!("gateway ready, entering main loop");

    loop {
        tokio::select! {
            _ = rt.sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
                rt.mcp_registry.write().await.disconnect_all().await;
                if let Some(tx) = rt.tunnel_shutdown_tx.take() { tx.send(true).ok(); }
                if let Some(tx) = rt.discord_shutdown_tx.take() { tx.send(true).ok(); }
                if let Some(tx) = rt.telegram_shutdown_tx.take() { tx.send(true).ok(); }
                rt.shutdown_tx.send(true).ok();
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
                    rt.mcp_registry.write().await.disconnect_all().await;
                    rt.shutdown_tx.send(true).ok();
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
                let mem = MemorySubsystems {
                    observer: &rt.observer, reflector: &rt.reflector,
                    search_index: &rt.search_index, layout: &rt.layout,
                    vector_store: rt.vector_store.as_ref(),
                    embedding_provider: rt.embedding_provider.as_ref(),
                };
                execute_observation(&mem, &mut rt.agent).await;
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
