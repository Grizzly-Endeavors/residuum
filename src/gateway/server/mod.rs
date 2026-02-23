//! WebSocket gateway server and main event loop.
//!
//! Accepts WebSocket connections from multiple clients and routes messages
//! through a single agent instance. All messages are forwarded to all clients;
//! verbose filtering is handled client-side.

mod context;
mod cron;
mod memory;
mod startup;
mod ws;

use std::sync::Arc;

use axum::routing::get;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Duration;

use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, SkillsContext};
use crate::channels::types::RoutedMessage;
use crate::config::Config;
use crate::cron::store::CronStore;
use crate::error::IronclawError;
use crate::memory::observer::{ObserveAction, Observer};
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::memory::types::Visibility;
use crate::models::ModelProvider;
use crate::projects::activation::SharedProjectState;
use crate::pulse::executor::execute_pulse;
use crate::pulse::scheduler::PulseScheduler;
use crate::pulse::types::AlertLevel;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::display::{BroadcastDisplay, ChannelAwareDisplay};
use super::protocol::ServerMessage;

use context::{build_project_context_strings, build_skill_context_strings, project_context_label};
use cron::run_due_cron_jobs_gateway;
use memory::{
    execute_observation, persist_and_check_thresholds, run_forced_observe, run_forced_reflect,
};
use ws::ws_handler;

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
    /// Reload requested; caller should re-run with fresh config.
    Reload,
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
struct GatewayState {
    inbound_tx: mpsc::Sender<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_sender: tokio::sync::watch::Sender<bool>,
    observe_notify: Arc<tokio::sync::Notify>,
    reflect_notify: Arc<tokio::sync::Notify>,
}

/// All state needed by the main event loop.
struct GatewayRuntime {
    // Subsystems (from initialization)
    layout: WorkspaceLayout,
    tz: chrono_tz::Tz,
    agent: Agent,
    observer: Observer,
    reflector: Reflector,
    search_index: Arc<MemoryIndex>,
    cron_store: Arc<tokio::sync::Mutex<CronStore>>,
    cron_notify: Arc<tokio::sync::Notify>,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    pulse_provider: Box<dyn ModelProvider>,
    cron_provider: Box<dyn ModelProvider>,
    pulse_enabled: bool,
    cron_enabled: bool,
    // Runtime channels + handles
    inbound_rx: mpsc::Receiver<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    broadcast_display: BroadcastDisplay,
    reload_rx: tokio::sync::watch::Receiver<bool>,
    observe_notify: Arc<tokio::sync::Notify>,
    reflect_notify: Arc<tokio::sync::Notify>,
    server_handle: tokio::task::JoinHandle<()>,
    pulse_scheduler: PulseScheduler,
}

/// Apply an `ObserveAction` to the current observe deadline.
///
/// Returns `true` if observation should fire immediately (`ForceNow`).
fn apply_observe_action(
    action: ObserveAction,
    observe_deadline: &mut Option<tokio::time::Instant>,
    cooldown_secs: u64,
) -> bool {
    match action {
        ObserveAction::ForceNow => {
            *observe_deadline = None;
            true
        }
        ObserveAction::StartCooldown => {
            *observe_deadline =
                Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs(cooldown_secs));
            false
        }
        ObserveAction::None => false,
    }
}

/// Start the WebSocket gateway server and run the main event loop.
///
/// Initializes all subsystems, spawns the axum WebSocket server, then enters
/// the event loop via `run_event_loop`.
///
/// # Errors
///
/// Returns `IronclawError` if initialization fails or the server cannot bind.
#[expect(
    clippy::too_many_lines,
    reason = "wires channels, server spawn, discord adapter, and GatewayRuntime assembly; each section is a distinct setup step"
)]
pub async fn run_gateway(cfg: Config) -> Result<GatewayExit, IronclawError> {
    let parts = startup::initialize(&cfg).await?;

    let (inbound_tx, inbound_rx) = mpsc::channel::<RoutedMessage>(32);
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ServerMessage>(256);
    let (reload_sender, reload_rx) = tokio::sync::watch::channel(false);
    let broadcast_display = BroadcastDisplay::new(broadcast_tx.clone());
    let observe_notify = Arc::new(tokio::sync::Notify::new());
    let reflect_notify = Arc::new(tokio::sync::Notify::new());

    // Clone senders for additional adapters before moving into GatewayState
    #[cfg(feature = "discord")]
    let discord_inbound_tx = inbound_tx.clone();
    let webhook_inbound_tx = inbound_tx.clone();
    #[cfg(feature = "discord")]
    let discord_reload_sender = reload_sender.clone();

    let state = GatewayState {
        inbound_tx,
        broadcast_tx: broadcast_tx.clone(),
        reload_sender,
        observe_notify: Arc::clone(&observe_notify),
        reflect_notify: Arc::clone(&reflect_notify),
    };

    let webhook_router = if cfg.webhook.enabled {
        let webhook_state = crate::channels::webhook::WebhookState {
            inbound_tx: webhook_inbound_tx,
            secret: cfg.webhook.secret.clone(),
        };
        Some(
            axum::Router::new()
                .route(
                    "/webhook",
                    axum::routing::post(crate::channels::webhook::webhook_handler),
                )
                .with_state(webhook_state),
        )
    } else {
        drop(webhook_inbound_tx);
        None
    };

    let mut app = axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);
    if let Some(wh) = webhook_router {
        app = app.merge(wh);
    }

    let addr = cfg.gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| IronclawError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");

    let mut shutdown_rx = reload_rx.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|v| *v).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    });

    // Spawn Discord adapter if configured
    #[cfg(feature = "discord")]
    if let Some(ref discord_cfg) = cfg.discord {
        let discord = crate::channels::discord::DiscordChannel::new(
            discord_cfg.clone(),
            discord_inbound_tx,
            cfg.workspace_dir.clone(),
            discord_reload_sender,
            Arc::clone(&observe_notify),
            Arc::clone(&reflect_notify),
        );
        tokio::spawn(async move {
            if let Err(e) = discord.start().await {
                tracing::error!(error = %e, "discord channel failed");
            }
        });
        tracing::info!("discord channel started (DM-only mode)");
    }

    let rt = GatewayRuntime {
        layout: parts.layout,
        tz: parts.tz,
        agent: parts.agent,
        observer: parts.observer,
        reflector: parts.reflector,
        search_index: parts.search_index,
        cron_store: parts.cron_store,
        cron_notify: parts.cron_notify,
        project_state: parts.project_state,
        skill_state: parts.skill_state,
        pulse_provider: parts.pulse_provider,
        cron_provider: parts.cron_provider,
        pulse_enabled: parts.pulse_enabled,
        cron_enabled: parts.cron_enabled,
        inbound_rx,
        broadcast_tx,
        broadcast_display,
        reload_rx,
        observe_notify,
        reflect_notify,
        server_handle,
        pulse_scheduler: PulseScheduler::new(),
    };

    Ok(run_event_loop(rt).await)
}

/// Run the main gateway event loop.
///
/// Processes inbound messages, pulse ticks, cron ticks, and memory pipeline
/// signals until shutdown or reload is requested.
#[expect(
    clippy::too_many_lines,
    reason = "8-arm select! loop; each arm is a distinct event source and cannot be split further"
)]
async fn run_event_loop(mut rt: GatewayRuntime) -> GatewayExit {
    let mut pulse_tick = tokio::time::interval(Duration::from_secs(60));
    let mut cron_tick = tokio::time::interval(Duration::from_secs(30));
    pulse_tick.tick().await; // skip first tick
    let mut observe_deadline: Option<tokio::time::Instant> = None;

    tracing::info!("gateway ready, entering main loop");

    loop {
        tokio::select! {
            // ── Reload signal ─────────────────────────────────────────────
            _ = rt.reload_rx.changed() => {
                tracing::info!("reloading configuration");
                rt.server_handle.abort();
                return GatewayExit::Reload;
            }

            // ── Inbound messages (from any channel) ──────────────────────
            msg = rt.inbound_rx.recv() => {
                let Some(routed) = msg else {
                    tracing::info!("inbound channel closed, shutting down");
                    rt.server_handle.abort();
                    break;
                };

                let reply_id = routed.message.id.clone();
                let origin = routed.message.origin.clone();

                if rt.broadcast_tx.send(ServerMessage::TurnStarted {
                    reply_to: reply_id.clone(),
                }).is_err() {
                    tracing::trace!("no broadcast receivers for turn_started");
                }

                let typing_guard = routed.reply.start_typing();
                let turn_display = ChannelAwareDisplay::new(
                    rt.broadcast_display.sender(),
                    Arc::clone(&routed.reply),
                );

                let before = rt.agent.message_count();

                let (idx_text, active_text) = build_project_context_strings(&rt.project_state).await;
                let projects_ctx = ProjectsContext {
                    index: idx_text.as_deref(),
                    active_context: active_text.as_deref(),
                };
                let (skill_idx_text, skill_active_text) = build_skill_context_strings(&rt.skill_state).await;
                let skills_ctx = SkillsContext {
                    index: skill_idx_text.as_deref(),
                    active_instructions: skill_active_text.as_deref(),
                };

                match rt.agent.process_message(&routed.message.content, &turn_display, Some(&origin), &projects_ctx, &skills_ctx).await {
                    Ok(texts) => {
                        drop(typing_guard);
                        for text in &texts {
                            routed.reply.send_response(text).await;
                        }
                        if origin.channel != "websocket" {
                            for text in texts {
                                if rt.broadcast_tx.send(ServerMessage::Response {
                                    reply_to: reply_id.clone(),
                                    content: text,
                                }).is_err() {
                                    tracing::trace!("no broadcast receivers for response");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        drop(typing_guard);
                        tracing::warn!(error = %e, "agent processing error");
                        if rt.broadcast_tx.send(ServerMessage::Error {
                            reply_to: Some(reply_id),
                            message: e.to_string(),
                        }).is_err() {
                            tracing::trace!("no broadcast receivers for error");
                        }
                    }
                }

                let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
                let project_context = project_context_label(&rt.project_state, &rt.layout).await;
                let action = persist_and_check_thresholds(
                    &new_messages, &project_context, Visibility::User,
                    &rt.observer, &rt.layout, rt.tz,
                ).await;
                if apply_observe_action(action, &mut observe_deadline, rt.observer.cooldown_secs()) {
                    execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent).await;
                }
            }

            // ── Pulse timer ───────────────────────────────────────────────
            _ = pulse_tick.tick(), if rt.pulse_enabled => {
                let now = crate::time::now_local(rt.tz);
                let due = rt.pulse_scheduler.due_pulses(now, &rt.layout.heartbeat_yml());

                let (pulse_idx_text, pulse_active_text) = build_project_context_strings(&rt.project_state).await;
                let pulse_projects_ctx = ProjectsContext {
                    index: pulse_idx_text.as_deref(),
                    active_context: pulse_active_text.as_deref(),
                };

                for pulse in &due {
                    match execute_pulse(
                        pulse, &rt.agent, &rt.layout.alerts_md(),
                        Some(&*rt.pulse_provider), &pulse_projects_ctx,
                    ).await {
                        Ok(result) => {
                            if !result.is_heartbeat_ok && !matches!(result.alert_level, AlertLevel::Low) {
                                let prefix = match result.alert_level {
                                    AlertLevel::High => format!("⚠ ALERT [{}]", result.pulse_name),
                                    AlertLevel::Medium | AlertLevel::Low => format!("pulse: {}", result.pulse_name),
                                };
                                if rt.broadcast_tx.send(ServerMessage::SystemEvent {
                                    source: prefix, content: result.response.clone(),
                                }).is_err() {
                                    tracing::trace!("no broadcast receivers for pulse event");
                                }
                            }
                            let pulse_context = project_context_label(&rt.project_state, &rt.layout).await;
                            let action = persist_and_check_thresholds(
                                &result.messages, &pulse_context, Visibility::Background,
                                &rt.observer, &rt.layout, rt.tz,
                            ).await;
                            if apply_observe_action(action, &mut observe_deadline, rt.observer.cooldown_secs()) {
                                execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent).await;
                            }
                        }
                        Err(e) => tracing::warn!(pulse = %pulse.name, error = %e, "pulse failed"),
                    }
                }
            }

            // ── Cron timer ────────────────────────────────────────────────
            _ = cron_tick.tick(), if rt.cron_enabled => {
                let action = run_due_cron_jobs_gateway(
                    &rt.cron_store, &mut rt.agent, &rt.observer, &rt.layout,
                    &rt.broadcast_tx, Some(&*rt.cron_provider), rt.tz, &rt.project_state,
                ).await;
                if apply_observe_action(action, &mut observe_deadline, rt.observer.cooldown_secs()) {
                    execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent).await;
                }
            }

            // ── Cron notify (tool mutation wakeup) ────────────────────────
            () = rt.cron_notify.notified(), if rt.cron_enabled => {
                let action = run_due_cron_jobs_gateway(
                    &rt.cron_store, &mut rt.agent, &rt.observer, &rt.layout,
                    &rt.broadcast_tx, Some(&*rt.cron_provider), rt.tz, &rt.project_state,
                ).await;
                if apply_observe_action(action, &mut observe_deadline, rt.observer.cooldown_secs()) {
                    execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent).await;
                }
            }

            // ── Observer cooldown deadline ────────────────────────────────
            () = async {
                match observe_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => {
                observe_deadline = None;
                execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent).await;
            }

            // ── Observe command wakeup ─────────────────────────────────────
            () = rt.observe_notify.notified() => {
                observe_deadline = None;
                run_forced_observe(
                    &rt.observer, &rt.reflector, &rt.search_index,
                    &rt.layout, &mut rt.agent, &rt.broadcast_tx,
                ).await;
            }

            // ── Reflect command wakeup ─────────────────────────────────────
            () = rt.reflect_notify.notified() => {
                run_forced_reflect(&rt.reflector, &rt.layout, &mut rt.agent, &rt.broadcast_tx).await;
            }
        }
    }

    GatewayExit::Shutdown
}
