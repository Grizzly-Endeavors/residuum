//! WebSocket gateway server and main event loop.
//!
//! Accepts WebSocket connections from multiple clients and routes messages
//! through a single agent instance. All messages are forwarded to all clients;
//! verbose filtering is handled client-side.

mod cron;
mod helpers;
mod memory;
mod spawn_helpers;
mod startup;
mod ws;

use std::sync::Arc;

use axum::routing::get;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Duration;

use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, PromptContext, SkillsContext, SubagentsContext};
use crate::agent::interrupt::Interrupt;
use crate::background::BackgroundTaskSpawner;
use crate::background::types::{BackgroundResult, ResultRouting, format_background_result};
use crate::channels::types::RoutedMessage;
use crate::config::Config;
use crate::cron::store::CronStore;
use crate::error::IronclawError;
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::{ObserveAction, Observer};
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::memory::types::Visibility;
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, Message};
use crate::notify::router::NotificationRouter;
use crate::notify::types::{Notification, TaskSource};
use crate::projects::activation::SharedProjectState;
use crate::pulse::executor::build_pulse_task;
use crate::pulse::scheduler::PulseScheduler;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::display::{BroadcastDisplay, ChannelAwareDisplay};
use super::protocol::ServerMessage;

use crate::agent::context::loading::{
    build_project_context_strings, build_skill_context_strings, build_subagents_context_string,
};
use cron::spawn_due_cron_jobs;
use helpers::project_context_label;
use memory::{
    execute_observation, persist_and_check_thresholds, run_forced_observe, run_forced_reflect,
};
use spawn_helpers::SpawnContext;
use ws::ws_handler;

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
    /// Reload requested; caller should re-run with fresh config.
    Reload,
}

/// A named command dispatched from any client channel to the server event loop.
pub struct ServerCommand {
    /// Command name (e.g. "observe", "reflect", "context").
    pub name: String,
    /// Optional argument text.
    pub args: Option<String>,
    /// Optional oneshot sender for commands that return a response (e.g. "context").
    pub reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
struct GatewayState {
    inbound_tx: mpsc::Sender<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_sender: tokio::sync::watch::Sender<bool>,
    command_tx: mpsc::Sender<ServerCommand>,
    inbox_dir: std::path::PathBuf,
    tz: chrono_tz::Tz,
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
    vector_store: Option<Arc<VectorStore>>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    cron_store: Arc<tokio::sync::Mutex<CronStore>>,
    cron_notify: Arc<tokio::sync::Notify>,
    mcp_registry: SharedMcpRegistry,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    pulse_enabled: bool,
    cron_enabled: bool,
    notification_router: NotificationRouter,
    background_spawner: Arc<BackgroundTaskSpawner>,
    background_result_rx: mpsc::Receiver<BackgroundResult>,
    spawn_context: Arc<SpawnContext>,
    // Runtime channels + handles
    inbound_rx: mpsc::Receiver<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    broadcast_display: BroadcastDisplay,
    reload_rx: tokio::sync::watch::Receiver<bool>,
    command_rx: mpsc::Receiver<ServerCommand>,
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
    let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);

    // Clone senders for additional adapters before moving into GatewayState
    #[cfg(feature = "discord")]
    let discord_inbound_tx = inbound_tx.clone();
    let webhook_inbound_tx = inbound_tx.clone();
    #[cfg(feature = "discord")]
    let discord_reload_sender = reload_sender.clone();
    #[cfg(feature = "discord")]
    let discord_command_tx = command_tx.clone();

    let state = GatewayState {
        inbound_tx,
        broadcast_tx: broadcast_tx.clone(),
        reload_sender,
        command_tx,
        inbox_dir: parts.layout.inbox_dir(),
        tz: parts.tz,
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
            discord_command_tx,
            parts.tz,
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
        vector_store: parts.vector_store,
        embedding_provider: parts.embedding_provider,
        cron_store: parts.cron_store,
        cron_notify: parts.cron_notify,
        mcp_registry: parts.mcp_registry,
        project_state: parts.project_state,
        skill_state: parts.skill_state,
        pulse_enabled: parts.pulse_enabled,
        cron_enabled: parts.cron_enabled,
        notification_router: parts.notification_router,
        background_spawner: parts.background_spawner,
        background_result_rx: parts.background_result_rx,
        spawn_context: parts.spawn_context,
        inbound_rx,
        broadcast_tx,
        broadcast_display,
        reload_rx,
        command_rx,
        server_handle,
        pulse_scheduler: PulseScheduler::new(),
    };

    Ok(run_event_loop(rt).await)
}

/// Handle a background task result: route through NOTIFY.yml or direct channels.
///
/// When a result targets the agent feed, the formatted message is persisted to
/// `recent_messages.json` and injected into the agent's conversation history
/// immediately — no longer deferred to the next user turn.
///
/// Returns `true` if observation should fire immediately (token threshold exceeded).
#[expect(
    clippy::too_many_arguments,
    reason = "threads subsystem references from the event loop for immediate persistence"
)]
async fn handle_background_result(
    result: BackgroundResult,
    router: &NotificationRouter,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    observer: &Observer,
    project_state: &SharedProjectState,
    tz: chrono_tz::Tz,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    // Pulse HEARTBEAT_OK results are silently logged — no routing, no agent events
    if matches!(result.source, TaskSource::Pulse) && result.summary.contains("HEARTBEAT_OK") {
        tracing::info!(task = %result.task_name, "pulse check: HEARTBEAT_OK");
        return false;
    }

    let formatted = format_background_result(&result);

    let should_inject = match &result.routing {
        ResultRouting::Notify => {
            let notification = Notification {
                task_name: result.task_name.clone(),
                summary: result.summary.clone(),
                source: result.source,
                timestamp: result.timestamp,
            };

            let outcome = router.route(&notification, &layout.notify_yml()).await;

            tracing::info!(
                task = %result.task_name,
                agent_wake = outcome.agent_wake,
                agent_feed = outcome.agent_feed,
                inbox = outcome.inbox,
                external_count = outcome.external_dispatched.len(),
                "background result routed via NOTIFY.yml"
            );

            outcome.agent_wake || outcome.agent_feed
        }
        ResultRouting::Direct(channels) => {
            let mut agent_inject = false;
            for channel_name in channels {
                match channel_name.as_str() {
                    "agent_wake" | "agent_feed" => agent_inject = true,
                    "inbox" => {
                        let notification = Notification {
                            task_name: result.task_name.clone(),
                            summary: result.summary.clone(),
                            source: result.source,
                            timestamp: result.timestamp,
                        };
                        router.route(&notification, &layout.notify_yml()).await;
                    }
                    _ => {
                        tracing::debug!(
                            channel = channel_name,
                            task = %result.task_name,
                            "direct channel routing (external channels not yet supported for direct)"
                        );
                    }
                }
            }
            agent_inject
        }
    };

    if !should_inject {
        return false;
    }

    // Persist immediately so the message survives restarts
    let sys_msg = Message::system(&formatted);
    let project_ctx = project_context_label(project_state, layout).await;
    let action = persist_and_check_thresholds(
        &[sys_msg],
        &project_ctx,
        Visibility::Background,
        observer,
        layout,
        tz,
    )
    .await;

    let force = apply_observe_action(action, observe_deadline, observer.cooldown_secs());

    // Inject into LLM context
    agent.inject_system_message(formatted);

    force
}

/// Dispatch a named server command from any client channel.
async fn handle_server_command(
    cmd: ServerCommand,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match cmd.name.as_str() {
        "observe" => {
            *observe_deadline = None;
            run_forced_observe(
                &rt.observer,
                &rt.reflector,
                &rt.search_index,
                &rt.layout,
                &mut rt.agent,
                &rt.broadcast_tx,
                rt.vector_store.as_ref(),
                rt.embedding_provider.as_ref(),
            )
            .await;
        }
        "reflect" => {
            run_forced_reflect(&rt.reflector, &rt.layout, &mut rt.agent, &rt.broadcast_tx).await;
        }
        "context" => {
            let (idx_text, active_text) = build_project_context_strings(&rt.project_state).await;
            let (skill_idx_text, skill_active_text) =
                build_skill_context_strings(&rt.skill_state).await;
            let subagents_idx_text =
                build_subagents_context_string(&rt.layout.subagents_dir()).await;
            let prompt_ctx = PromptContext {
                projects: ProjectsContext {
                    index: idx_text.as_deref(),
                    active_context: active_text.as_deref(),
                },
                skills: SkillsContext {
                    index: skill_idx_text.as_deref(),
                    active_instructions: skill_active_text.as_deref(),
                },
                subagents: SubagentsContext {
                    index: subagents_idx_text.as_deref(),
                },
            };
            let bd = rt.agent.context_breakdown(&prompt_ctx).await;
            let msg = format!(
                "[context]\n  identity:          ~{} tokens\n  observation log:   ~{} tokens\n  subagents index:   ~{} tokens\n  projects index:    ~{} tokens\n  active project:    ~{} tokens\n  skills index:      ~{} tokens\n  active skills:     ~{} tokens\n  system tools:      ~{} tokens\n  mcp tools:         ~{} tokens\n  message history:   ~{} tokens ({} messages)",
                bd.identity_tokens,
                bd.observation_log_tokens,
                bd.subagents_index_tokens,
                bd.projects_index_tokens,
                bd.active_project_tokens,
                bd.skills_index_tokens,
                bd.active_skills_tokens,
                bd.system_tool_tokens,
                bd.mcp_tool_tokens,
                bd.history_tokens,
                bd.history_count,
            );
            if let Some(tx) = cmd.reply_tx {
                tx.send(msg.clone()).ok();
            }
            rt.broadcast_tx
                .send(ServerMessage::Notice { message: msg })
                .ok();
        }
        unknown => {
            rt.broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("unknown server command: {unknown}"),
                })
                .ok();
        }
    }
}

/// Run the main gateway event loop.
///
/// Processes inbound messages, pulse ticks, cron ticks, and memory pipeline
/// signals until shutdown or reload is requested.
#[expect(
    clippy::too_many_lines,
    reason = "7-arm select! loop; each arm is a distinct event source and cannot be split further"
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
                rt.mcp_registry.write().await.disconnect_all().await;
                rt.server_handle.abort();
                return GatewayExit::Reload;
            }

            // ── Inbound messages (from any channel) ──────────────────────
            msg = rt.inbound_rx.recv() => {
                let Some(routed) = msg else {
                    tracing::info!("inbound channel closed, shutting down");
                    rt.mcp_registry.write().await.disconnect_all().await;
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
                let (skill_idx_text, skill_active_text) = build_skill_context_strings(&rt.skill_state).await;
                let subagents_idx_text = build_subagents_context_string(&rt.layout.subagents_dir()).await;
                let prompt_ctx = PromptContext {
                    projects: ProjectsContext {
                        index: idx_text.as_deref(),
                        active_context: active_text.as_deref(),
                    },
                    skills: SkillsContext {
                        index: skill_idx_text.as_deref(),
                        active_instructions: skill_active_text.as_deref(),
                    },
                    subagents: SubagentsContext {
                        index: subagents_idx_text.as_deref(),
                    },
                };

                let turn_result = {
                    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);
                    let mut turn = std::pin::pin!(
                        rt.agent.process_message(
                            &routed.message.content,
                            &turn_display,
                            Some(&origin),
                            &prompt_ctx,
                            &mut interrupt_rx,
                        )
                    );

                    loop {
                        tokio::select! {
                            result = &mut turn => break result,
                            next_msg = rt.inbound_rx.recv() => {
                                if let Some(next_routed) = next_msg {
                                    drop(interrupt_tx.try_send(
                                        Interrupt::UserMessage(next_routed.message)
                                    ));
                                }
                            }
                            bg_result = rt.background_result_rx.recv() => {
                                if let Some(result) = bg_result {
                                    drop(interrupt_tx.try_send(
                                        Interrupt::BackgroundResult(result)
                                    ));
                                }
                            }
                            _ = rt.reload_rx.changed() => {
                                rt.mcp_registry.write().await.disconnect_all().await;
                                rt.server_handle.abort();
                                return GatewayExit::Reload;
                            }
                        }
                    }
                };

                match turn_result {
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
                    execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent, rt.vector_store.as_ref(), rt.embedding_provider.as_ref()).await;
                }
            }

            // ── Pulse timer ───────────────────────────────────────────────
            _ = pulse_tick.tick(), if rt.pulse_enabled => {
                let now = crate::time::now_local(rt.tz);
                let due = rt.pulse_scheduler.due_pulses(now, &rt.layout.heartbeat_yml());

                for pulse in &due {
                    let task = build_pulse_task(pulse);
                    let tier = match &task.execution {
                        crate::background::types::Execution::SubAgent(cfg) => cfg.model_tier,
                        crate::background::types::Execution::Script(_) => crate::config::BackgroundModelTier::Small,
                    };
                    match spawn_helpers::build_spawn_resources(
                        &rt.spawn_context, &tier,
                        &rt.project_state, &rt.skill_state,
                        Arc::clone(&rt.mcp_registry),
                        None,
                    ).await {
                        Ok(resources) => {
                            if let Err(e) = rt.background_spawner.spawn(task, Some(resources)).await {
                                tracing::warn!(pulse = %pulse.name, error = %e, "failed to spawn pulse task");
                            }
                        }
                        Err(e) => tracing::warn!(pulse = %pulse.name, error = %e, "failed to build pulse resources"),
                    }
                }
            }

            // ── Cron timer ────────────────────────────────────────────────
            _ = cron_tick.tick(), if rt.cron_enabled => {
                spawn_due_cron_jobs(
                    &rt.cron_store, &rt.layout,
                    &rt.spawn_context, &rt.project_state, &rt.skill_state,
                    &rt.mcp_registry, &rt.background_spawner, rt.tz,
                ).await;
            }

            // ── Cron notify (tool mutation wakeup) ────────────────────────
            () = rt.cron_notify.notified(), if rt.cron_enabled => {
                spawn_due_cron_jobs(
                    &rt.cron_store, &rt.layout,
                    &rt.spawn_context, &rt.project_state, &rt.skill_state,
                    &rt.mcp_registry, &rt.background_spawner, rt.tz,
                ).await;
            }

            // ── Observer cooldown deadline ────────────────────────────────
            () = async {
                match observe_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => {
                observe_deadline = None;
                execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent, rt.vector_store.as_ref(), rt.embedding_provider.as_ref()).await;
            }

            // ── Background task results ──────────────────────────────────
            bg_result = rt.background_result_rx.recv() => {
                if let Some(result) = bg_result {
                    let force = handle_background_result(
                        result, &rt.notification_router, &rt.layout,
                        &mut rt.agent, &rt.observer, &rt.project_state,
                        rt.tz, &mut observe_deadline,
                    ).await;
                    if force {
                        execute_observation(&rt.observer, &rt.reflector, &rt.search_index, &rt.layout, &mut rt.agent, rt.vector_store.as_ref(), rt.embedding_provider.as_ref()).await;
                    }
                }
            }

            // ── Server commands (observe, reflect, context, etc.) ────────
            cmd = rt.command_rx.recv() => {
                if let Some(cmd) = cmd {
                    handle_server_command(cmd, &mut rt, &mut observe_deadline).await;
                }
            }
        }
    }

    GatewayExit::Shutdown
}
