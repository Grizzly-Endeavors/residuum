//! WebSocket gateway server and main event loop.
//!
//! Accepts WebSocket connections from multiple clients and routes messages
//! through a single agent instance. All messages are forwarded to all clients;
//! verbose filtering is handled client-side.

mod actions;
mod helpers;
mod idle;
mod memory;
mod reload;
pub mod setup;
mod spawn_helpers;
mod startup;
mod watcher;
pub(crate) mod web;
mod ws;

use std::sync::Arc;

use axum::routing::get;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Duration;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, PromptContext, SkillsContext, SubagentsContext};
use crate::agent::interrupt::Interrupt;
use crate::background::BackgroundTaskSpawner;
use crate::background::types::{BackgroundResult, ResultRouting, format_background_result};
use crate::config::Config;
use crate::error::ResiduumError;
use crate::interfaces::types::{ReplyHandle, RoutedMessage};
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::{ObserveAction, Observer};
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::memory::types::Visibility;
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, Message, SharedHttpClient};
use crate::notify::router::NotificationRouter;
use crate::notify::types::{
    BuiltinChannel, ChannelTarget, Notification, TaskSource, parse_channel_list,
};
use crate::projects::activation::SharedProjectState;
use crate::pulse::executor::{PulseExecution, build_pulse_execution};
use crate::pulse::scheduler::PulseScheduler;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::protocol::ServerMessage;

use crate::agent::context::loading::{
    build_project_context_strings, build_skill_context_strings, build_subagents_context_string,
};
use crate::background::spawn_context::load_preset_for_spawn;
use helpers::project_context_label;
use memory::{
    MemorySubsystems, execute_observation, persist_and_check_thresholds, run_forced_observe,
    run_forced_reflect,
};
use spawn_helpers::SpawnContext;
use ws::ws_handler;

/// Describes what kind of configuration reload was requested.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ReloadSignal {
    /// No reload pending.
    #[default]
    None,
    /// Full root config reload (config.toml changed).
    Root,
    /// Workspace-level reload (mcp.json or channels.toml changed).
    Workspace,
}

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
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

/// Long-lived core that owns shared communication channels.
///
/// Created once at startup and persists across configuration reloads.
/// The senders are cloned into adapters, the web server, and event loop state.
pub(crate) struct GatewayCore {
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    /// Dedicated shutdown signal for the HTTP server (not tied to reload).
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub config_dir: std::path::PathBuf,
}

/// Receiver halves consumed by the event loop.
pub(crate) struct CoreReceivers {
    pub inbound: mpsc::Receiver<RoutedMessage>,
    pub reload: tokio::sync::watch::Receiver<ReloadSignal>,
    pub command: mpsc::Receiver<ServerCommand>,
}

impl GatewayCore {
    /// Create a new gateway core with fresh channels.
    ///
    /// # Errors
    /// Returns `ResiduumError` if the SIGTERM handler cannot be registered.
    pub fn new(config_dir: std::path::PathBuf) -> (Self, CoreReceivers) {
        let (inbound_tx, inbound_rx) = mpsc::channel::<RoutedMessage>(32);
        let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ServerMessage>(256);
        let (reload_tx, reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let core = Self {
            inbound_tx,
            broadcast_tx,
            reload_tx,
            command_tx,
            shutdown_tx,
            config_dir,
        };
        let receivers = CoreReceivers {
            inbound: inbound_rx,
            reload: reload_rx,
            command: command_rx,
        };
        (core, receivers)
    }
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
struct GatewayState {
    inbound_tx: mpsc::Sender<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    inbox_dir: std::path::PathBuf,
    tz: chrono_tz::Tz,
}

/// All state needed by the main event loop.
struct GatewayRuntime {
    // Current running config (for diffing on reload)
    cfg: Config,
    // Subsystems (from initialization)
    layout: WorkspaceLayout,
    tz: chrono_tz::Tz,
    agent: Agent,
    observer: Observer,
    reflector: Reflector,
    search_index: Arc<MemoryIndex>,
    vector_store: Option<Arc<VectorStore>>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: Arc<tokio::sync::Notify>,
    mcp_registry: SharedMcpRegistry,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    pulse_enabled: bool,
    notification_router: Arc<NotificationRouter>,
    http_client: SharedHttpClient,
    background_spawner: Arc<BackgroundTaskSpawner>,
    background_result_rx: mpsc::Receiver<BackgroundResult>,
    spawn_context: Arc<SpawnContext>,
    // Runtime channels + handles
    inbound_rx: mpsc::Receiver<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_rx: tokio::sync::watch::Receiver<ReloadSignal>,
    command_rx: mpsc::Receiver<ServerCommand>,
    /// Kept alive so the HTTP server task isn't dropped; shut down via `shutdown_tx`.
    server_handle: tokio::task::JoinHandle<()>,
    pulse_scheduler: PulseScheduler,
    /// SIGTERM signal listener for daemon stop support.
    sigterm: tokio::signal::unix::Signal,
    /// Dedicated shutdown signal for the HTTP server.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Path to the config directory (for backup/rollback during reload).
    config_dir: std::path::PathBuf,
    /// Most recent reply handle from a user message. Used by wake turns to
    /// deliver responses to the channel the user last interacted from.
    last_reply: Option<Arc<dyn ReplyHandle>>,
    /// Unsolicited send handles keyed by interface name. Populated on first
    /// message from each interface for use during idle channel switching.
    unsolicited_handles: std::collections::HashMap<String, Arc<dyn ReplyHandle>>,
    /// When the last user message was received (for idle deadline recalculation on reload).
    last_user_message_instant: Option<tokio::time::Instant>,
    // Adapter lifecycle handles
    discord_handle: Option<tokio::task::JoinHandle<()>>,
    telegram_handle: Option<tokio::task::JoinHandle<()>>,
    discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Cloned core senders for rebuilding adapters on reload.
    inbound_tx: mpsc::Sender<RoutedMessage>,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    /// Shared path policy for updating blocked paths on reload.
    path_policy: crate::tools::SharedPathPolicy,
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

/// Build the axum `Router` for the HTTP/WS server.
///
/// Extracted for reuse during gateway port rebinding on config reload.
fn build_gateway_app(
    state: GatewayState,
    cfg: &Config,
    config_api_state: web::ConfigApiState,
) -> axum::Router {
    let webhook_router = cfg.webhook.enabled.then(|| {
        let webhook_state = crate::interfaces::webhook::WebhookState {
            inbound_tx: state.inbound_tx.clone(),
            secret: cfg.webhook.secret.clone(),
        };
        axum::Router::new()
            .route(
                "/webhook",
                axum::routing::post(crate::interfaces::webhook::webhook_handler),
            )
            .with_state(webhook_state)
    });

    let mut app = axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);
    if let Some(wh) = webhook_router {
        app = app.merge(wh);
    }
    app.merge(web::config_api_router(config_api_state))
        .fallback(web::static_handler)
}

/// Bind the HTTP server and spawn it as a background task.
///
/// # Errors
/// Returns `ResiduumError` if the listener cannot bind to the configured address.
async fn spawn_http_server(
    cfg: &Config,
    app: axum::Router,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> Result<tokio::task::JoinHandle<()>, ResiduumError> {
    let addr = cfg.gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");
    if cfg.gateway.bind != "127.0.0.1" && cfg.gateway.bind != "localhost" {
        tracing::warn!(
            bind = %cfg.gateway.bind,
            "web UI is exposed on a non-loopback address with no authentication"
        );
    }

    let mut shutdown_rx = shutdown_tx.subscribe();
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|v| *v).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    });
    Ok(handle)
}

/// Start the WebSocket gateway server and run the main event loop.
///
/// Initializes all subsystems, spawns the axum WebSocket server, then enters
/// the event loop via `run_event_loop`.
///
/// # Errors
///
/// Returns `ResiduumError` if initialization fails or the server cannot bind.
pub async fn run_gateway(cfg: Config) -> Result<GatewayExit, ResiduumError> {
    backup_config(&cfg.config_dir);

    let parts = startup::initialize(&cfg).await?;
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

    let state = GatewayState {
        inbound_tx: core.inbound_tx,
        broadcast_tx: core.broadcast_tx.clone(),
        reload_tx: core.reload_tx.clone(),
        command_tx: core.command_tx,
        inbox_dir: parts.layout.inbox_dir(),
        tz: parts.tz,
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

/// Outcome of handling a background task result.
struct BackgroundResultOutcome {
    /// Whether observation should fire immediately (token threshold exceeded).
    force_observe: bool,
    /// Whether the agent should start an autonomous wake turn.
    wake_requested: bool,
}

/// Bundled context for handling background task results.
struct BackgroundContext<'a> {
    router: &'a NotificationRouter,
    layout: &'a WorkspaceLayout,
    observer: &'a Observer,
    project_state: &'a SharedProjectState,
    tz: chrono_tz::Tz,
}

/// Handle a background task result: route to channels.
///
/// When a result targets the agent feed or wake, the formatted message is persisted to
/// `recent_messages.json` and injected into the agent's conversation history
/// immediately — no longer deferred to the next user turn.
async fn handle_background_result(
    result: BackgroundResult,
    ctx: &BackgroundContext<'_>,
    agent: &mut Agent,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> BackgroundResultOutcome {
    let no_action = BackgroundResultOutcome {
        force_observe: false,
        wake_requested: false,
    };

    // Pulse HEARTBEAT_OK results are silently logged — no routing, no agent events
    if matches!(result.source, TaskSource::Pulse) && result.summary.contains("HEARTBEAT_OK") {
        tracing::info!(task = %result.task_name, "pulse check: HEARTBEAT_OK");
        return no_action;
    }

    let formatted = format_background_result(&result);

    let ResultRouting::Direct(channels) = &result.routing;
    let targets = parse_channel_list(channels);
    let (should_inject, wake) = {
        let mut agent_inject = false;
        let mut wake_requested = false;
        for target in &targets {
            match target {
                ChannelTarget::Builtin(BuiltinChannel::AgentWake) => {
                    agent_inject = true;
                    wake_requested = true;
                }
                ChannelTarget::Builtin(BuiltinChannel::AgentFeed) => agent_inject = true,
                ChannelTarget::Builtin(BuiltinChannel::Inbox) => {
                    let notification = Notification {
                        task_name: result.task_name.clone(),
                        summary: result.summary.clone(),
                        source: result.source,
                        timestamp: result.timestamp,
                    };
                    ctx.router.deliver_to_inbox(&notification).await;
                }
                ChannelTarget::External(ext_name) => {
                    tracing::debug!(
                        channel = %ext_name,
                        task = %result.task_name,
                        "direct channel routing (external channels not yet supported for direct)"
                    );
                }
            }
        }
        (agent_inject, wake_requested)
    };

    if !should_inject {
        return no_action;
    }

    // Persist immediately so the message survives restarts
    let sys_msg = Message::system(&formatted);
    let project_ctx = project_context_label(ctx.project_state, ctx.layout).await;
    let action = persist_and_check_thresholds(
        &[sys_msg],
        &project_ctx,
        Visibility::Background,
        ctx.observer,
        ctx.layout,
        ctx.tz,
    )
    .await;

    let force = apply_observe_action(action, observe_deadline, ctx.observer.cooldown_secs());

    // Inject into LLM context
    agent.inject_system_message(formatted);

    BackgroundResultOutcome {
        force_observe: force,
        wake_requested: wake,
    }
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
            let mem = MemorySubsystems {
                observer: &rt.observer,
                reflector: &rt.reflector,
                search_index: &rt.search_index,
                layout: &rt.layout,
                vector_store: rt.vector_store.as_ref(),
                embedding_provider: rt.embedding_provider.as_ref(),
            };
            run_forced_observe(&mem, &mut rt.agent, &rt.broadcast_tx).await;
        }
        "reflect" => {
            run_forced_reflect(&rt.reflector, &rt.layout, &mut rt.agent, &rt.broadcast_tx).await;
        }
        "context" => {
            let ctx_strings =
                load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
            let prompt_ctx = ctx_strings.as_prompt_context();
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

/// Process leftover interrupts that arrived during an agent turn but weren't consumed.
///
/// Background results are routed and observed; user messages are injected into
/// the agent's conversation. Returns `true` if any leftover triggered a wake request
/// (callers decide whether to act on it).
async fn process_leftover_interrupts(
    leftovers: Vec<Interrupt>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let mut wake = false;
    for intr in leftovers {
        match intr {
            Interrupt::BackgroundResult(bg_leftover) => {
                let bg_ctx = BackgroundContext {
                    router: &rt.notification_router,
                    layout: &rt.layout,
                    observer: &rt.observer,
                    project_state: &rt.project_state,
                    tz: rt.tz,
                };
                let bg_outcome =
                    handle_background_result(bg_leftover, &bg_ctx, &mut rt.agent, observe_deadline)
                        .await;
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
                if bg_outcome.wake_requested {
                    wake = true;
                }
            }
            Interrupt::UserMessage(leftover_msg) => {
                rt.agent.inject_user_message(leftover_msg.content);
            }
        }
    }
    wake
}

/// Drain remaining interrupts from an interrupt channel after a turn completes.
fn drain_interrupts(interrupt_rx: &mut mpsc::Receiver<Interrupt>) -> Vec<Interrupt> {
    let mut leftovers = Vec::new();
    while let Ok(intr) = interrupt_rx.try_recv() {
        leftovers.push(intr);
    }
    leftovers
}

/// Raw prompt context strings for constructing a `PromptContext`.
///
/// Held as owned `Option<String>` so that `PromptContext` can borrow via `as_deref()`.
struct PromptContextStrings {
    proj_index: Option<String>,
    proj_active: Option<String>,
    skill_index: Option<String>,
    skill_active: Option<String>,
    subagents_index: Option<String>,
}

impl PromptContextStrings {
    /// Build a borrowed `PromptContext` from these owned strings.
    fn as_prompt_context(&self) -> PromptContext<'_> {
        PromptContext {
            projects: ProjectsContext {
                index: self.proj_index.as_deref(),
                active_context: self.proj_active.as_deref(),
            },
            skills: SkillsContext {
                index: self.skill_index.as_deref(),
                active_instructions: self.skill_active.as_deref(),
            },
            subagents: SubagentsContext {
                index: self.subagents_index.as_deref(),
            },
        }
    }
}

/// Load prompt context strings from project, skill, and subagent state.
async fn load_prompt_context_strings(
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    layout: &WorkspaceLayout,
) -> PromptContextStrings {
    let (proj_index, proj_active) = build_project_context_strings(project_state).await;
    let (skill_index, skill_active) = build_skill_context_strings(skill_state).await;
    let subagents_index = build_subagents_context_string(&layout.subagents_dir()).await;
    PromptContextStrings {
        proj_index,
        proj_active,
        skill_index,
        skill_active,
        subagents_index,
    }
}

/// Run an autonomous agent wake turn triggered by a background result.
///
/// Follows the same pattern as the inbound message handler but does not push
/// a user message — uses `run_wake_turn` which injects a system kickoff.
/// Broadcasts responses with `reply_to: "wake"` and persists messages with
/// `Visibility::Background`.
///
/// Returns `Some(GatewayExit)` if a reload signal fires during the turn.
async fn run_wake_turn_handler(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    let Some(reply) = rt.last_reply.as_ref().map(Arc::clone) else {
        tracing::warn!("wake turn requested but no channel has connected yet, skipping");
        return None;
    };

    tracing::info!("starting autonomous wake turn from background result");

    let before = rt.agent.message_count();

    let ctx_strings =
        load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let typing_guard = reply.start_typing();
    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);

    let turn_result = {
        let mut turn = std::pin::pin!(rt.agent.run_wake_turn(
            &*reply,
            &prompt_ctx,
            &mut interrupt_rx,
        ));

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
                    tracing::info!("reload signal received during wake turn, deferring");
                }
            }
        }
    };

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    match turn_result {
        Ok(texts) => {
            drop(typing_guard);
            for text in &texts {
                reply.send_response(text).await;
            }
        }
        Err(e) => {
            drop(typing_guard);
            tracing::warn!(error = %e, "wake turn processing error");
            if rt
                .broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: Some("wake".to_string()),
                    message: e.to_string(),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for wake error");
            }
        }
    }

    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(rt, &new_messages, Visibility::Background, observe_deadline).await;

    // Don't recursively trigger wake turns from leftovers
    process_leftover_interrupts(leftover_interrupts, rt, observe_deadline).await;

    None
}

/// Inject scheduled action main-turn prompts and fire a single wake turn if any are due.
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

/// Bundled senders for spawning a chat adapter (Discord or Telegram).
struct AdapterSenders {
    inbound: mpsc::Sender<RoutedMessage>,
    reload: tokio::sync::watch::Sender<ReloadSignal>,
    command: mpsc::Sender<ServerCommand>,
}

/// Lifecycle handles returned from spawning chat adapters.
struct AdapterHandles {
    discord_handle: Option<tokio::task::JoinHandle<()>>,
    discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    telegram_handle: Option<tokio::task::JoinHandle<()>>,
    telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

/// Spawn Discord and Telegram adapters if configured.
fn spawn_adapters(
    cfg: &Config,
    discord: AdapterSenders,
    telegram: AdapterSenders,
    tz: chrono_tz::Tz,
) -> AdapterHandles {
    let (mut discord_handle, mut discord_shutdown_tx) = (None, None);
    if let Some(ref discord_cfg) = cfg.discord {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::discord::DiscordInterface::new(
            discord_cfg.clone(),
            discord.inbound,
            cfg.workspace_dir.clone(),
            discord.reload,
            discord.command,
            tz,
            rx,
        );
        discord_handle = Some(tokio::spawn(async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "discord interface failed");
            }
        }));
        discord_shutdown_tx = Some(tx);
        tracing::info!("discord interface started (DM-only mode)");
    }

    let (mut telegram_handle, mut telegram_shutdown_tx) = (None, None);
    if let Some(ref telegram_cfg) = cfg.telegram {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::telegram::TelegramInterface::new(
            telegram_cfg.clone(),
            telegram.inbound,
            cfg.workspace_dir.clone(),
            telegram.reload,
            telegram.command,
            tz,
            rx,
        );
        telegram_handle = Some(tokio::spawn(async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "telegram interface failed");
            }
        }));
        telegram_shutdown_tx = Some(tx);
        tracing::info!("telegram interface started (DM-only mode)");
    }

    AdapterHandles {
        discord_handle,
        discord_shutdown_tx,
        telegram_handle,
        telegram_shutdown_tx,
    }
}

/// Persist new messages and run observation if thresholds are exceeded.
async fn persist_and_maybe_observe(
    rt: &mut GatewayRuntime,
    new_messages: &[Message],
    visibility: Visibility,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    let project_ctx = project_context_label(&rt.project_state, &rt.layout).await;
    let action = persist_and_check_thresholds(
        new_messages,
        &project_ctx,
        visibility,
        &rt.observer,
        &rt.layout,
        rt.tz,
    )
    .await;
    if apply_observe_action(action, observe_deadline, rt.observer.cooldown_secs()) {
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
}

/// Handle an inbound user message: run agent turn, persist, observe, and process leftovers.
///
/// Returns `Some(GatewayExit)` if a shutdown-worthy event occurs during processing.
async fn handle_inbound_message(
    routed: RoutedMessage,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    let reply_id = routed.message.id.clone();
    let origin = routed.message.origin.clone();

    // TurnStarted is WS-specific protocol sugar
    if origin.interface == "websocket" {
        rt.broadcast_tx
            .send(ServerMessage::TurnStarted {
                reply_to: reply_id.clone(),
            })
            .ok();
    }

    rt.last_reply = Some(Arc::clone(&routed.reply));
    if let std::collections::hash_map::Entry::Vacant(e) =
        rt.unsolicited_handles.entry(origin.interface.clone())
        && let Some(h) = routed.reply.unsolicited_clone()
    {
        e.insert(h);
    }
    let typing_guard = routed.reply.start_typing();
    let before = rt.agent.message_count();

    let ctx_strings =
        load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);
    let turn_result = {
        let mut turn = std::pin::pin!(rt.agent.process_message(
            &routed.message.content,
            &*routed.reply,
            Some(&origin),
            &prompt_ctx,
            &mut interrupt_rx,
            &routed.message.images,
        ));
        loop {
            tokio::select! {
                result = &mut turn => break result,
                next_msg = rt.inbound_rx.recv() => {
                    if let Some(next_routed) = next_msg {
                        drop(interrupt_tx.try_send(Interrupt::UserMessage(next_routed.message)));
                    }
                }
                bg_result = rt.background_result_rx.recv() => {
                    if let Some(result) = bg_result {
                        drop(interrupt_tx.try_send(Interrupt::BackgroundResult(result)));
                    }
                }
                _ = rt.reload_rx.changed() => {
                    tracing::info!("reload signal received during active turn, deferring");
                }
            }
        }
    };

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    drop(typing_guard);
    match turn_result {
        Ok(texts) => {
            for text in &texts {
                routed.reply.send_response(text).await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "agent processing error");
            rt.broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: Some(reply_id),
                    message: e.to_string(),
                })
                .ok();
        }
    }

    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(rt, &new_messages, Visibility::User, observe_deadline).await;

    if process_leftover_interrupts(leftover_interrupts, rt, observe_deadline).await
        && let Some(exit) = run_wake_turn_handler(rt, observe_deadline).await
    {
        return Some(exit);
    }

    if !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }
    None
}

/// Back up `config.toml` and `providers.toml` before a reload attempt.
///
/// Best-effort: logs a warning on failure but never panics.
pub fn backup_config(config_dir: &std::path::Path) {
    for name in &["config.toml", "providers.toml"] {
        let src = config_dir.join(name);
        let dst = config_dir.join(format!("{name}.bak"));
        if src.exists() {
            if let Err(err) = std::fs::copy(&src, &dst) {
                tracing::warn!(file = %name, error = %err, "failed to back up before reload");
            } else {
                tracing::debug!(file = %name, "backed up to .bak");
            }
        }
    }
}

/// Restore `.bak` files for `config.toml` and `providers.toml` after a failed reload.
///
/// Returns `true` if at least one file was restored successfully.
pub fn rollback_config(config_dir: &std::path::Path) -> bool {
    let mut any_restored = false;
    for name in &["config.toml", "providers.toml"] {
        let backup = config_dir.join(format!("{name}.bak"));
        let target = config_dir.join(name);
        if !backup.exists() {
            continue;
        }
        match std::fs::copy(&backup, &target) {
            Ok(_) => {
                tracing::info!(file = %name, "restored from backup");
                any_restored = true;
            }
            Err(err) => {
                tracing::warn!(file = %name, error = %err, "failed to restore from backup");
            }
        }
    }
    if !any_restored {
        tracing::warn!("no config backups found, cannot rollback");
    }
    any_restored
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
            );
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

/// Handle a single pulse execution entry (main-turn or sub-agent).
async fn handle_pulse_execution(
    execution: PulseExecution,
    pulse_name: &str,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    match execution {
        PulseExecution::MainWakeTurn {
            pulse_name: _,
            prompt,
        } => {
            let formatted = format!("[Scheduled pulse: {pulse_name}]\n{prompt}");
            rt.agent.inject_system_message(formatted.clone());
            let msgs = [crate::models::Message::system(&formatted)];
            persist_and_maybe_observe(rt, &msgs, Visibility::Background, observe_deadline).await;
            true
        }
        PulseExecution::SubAgent {
            task,
            preset_name: Some(name),
        } => {
            match load_preset_for_spawn(
                &rt.layout.subagents_dir(),
                &name,
                crate::config::BackgroundModelTier::Small,
            )
            .await
            {
                Ok((tier, preset)) => {
                    let preset_arg = preset.as_ref().map(|(fm, body)| (fm, body.clone()));
                    match spawn_helpers::build_spawn_resources(
                        &rt.spawn_context,
                        &tier,
                        &rt.project_state,
                        &rt.skill_state,
                        Arc::clone(&rt.mcp_registry),
                        preset_arg,
                    )
                    .await
                    {
                        Ok(resources) => {
                            if let Err(e) = rt.background_spawner.spawn(task, Some(resources)).await
                            {
                                tracing::warn!(pulse = %pulse_name, error = %e, "failed to spawn pulse task with preset");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(pulse = %pulse_name, error = %e, "failed to build pulse resources with preset");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(pulse = %pulse_name, preset = %name, error = %e, "failed to load preset for pulse");
                }
            }
            false
        }
        PulseExecution::SubAgent {
            task,
            preset_name: None,
        } => {
            let crate::background::types::Execution::SubAgent(cfg) = &task.execution;
            let tier = cfg.model_tier;
            match spawn_helpers::build_spawn_resources(
                &rt.spawn_context,
                &tier,
                &rt.project_state,
                &rt.skill_state,
                Arc::clone(&rt.mcp_registry),
                None,
            )
            .await
            {
                Ok(resources) => {
                    if let Err(e) = rt.background_spawner.spawn(task, Some(resources)).await {
                        tracing::warn!(pulse = %pulse_name, error = %e, "failed to spawn pulse task");
                    }
                }
                Err(e) => {
                    tracing::warn!(pulse = %pulse_name, error = %e, "failed to build pulse resources");
                }
            }
            false
        }
    }
}

/// Handle a background task result in the event loop: route, observe, and optionally wake.
async fn handle_event_loop_bg_result(
    result: BackgroundResult,
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

/// Process all due pulses and optionally trigger a wake turn.
async fn handle_pulse_tick(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    let now = crate::time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
    let mut wake_requested = false;
    for pulse in &due {
        let name = pulse.name.clone();
        let exec = build_pulse_execution(pulse);
        if handle_pulse_execution(exec, &name, rt, observe_deadline).await {
            wake_requested = true;
        }
    }
    if wake_requested {
        return run_wake_turn_handler(rt, observe_deadline).await;
    }
    None
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
    use super::*;

    #[test]
    fn reload_signal_default_is_none() {
        let signal = ReloadSignal::default();
        assert_eq!(signal, ReloadSignal::None);
    }

    #[tokio::test]
    async fn core_channels_survive_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let (core, receivers) = GatewayCore::new(dir.path().to_path_buf());

        // Send a message through inbound before reload
        assert!(
            core.inbound_tx
                .send(crate::interfaces::types::RoutedMessage {
                    message: crate::interfaces::types::InboundMessage {
                        id: "test-1".to_string(),
                        content: "hello".to_string(),
                        origin: crate::interfaces::types::MessageOrigin {
                            interface: "test".to_string(),
                            sender_name: "tester".to_string(),
                            sender_id: "t1".to_string(),
                        },
                        timestamp: chrono::Utc::now(),
                        images: vec![],
                    },
                    reply: std::sync::Arc::new(crate::interfaces::websocket::WsReplyHandle::new(
                        core.broadcast_tx.clone(),
                        "test-1".to_string(),
                    ),),
                })
                .await
                .is_ok(),
            "inbound send should succeed before reload"
        );

        // Fire a reload signal
        core.reload_tx.send(ReloadSignal::Root).unwrap();

        // Channels still work after the reload signal
        let _broadcast_rx = core.broadcast_tx.subscribe();
        assert!(
            core.broadcast_tx
                .send(crate::gateway::protocol::ServerMessage::Pong)
                .is_ok(),
            "broadcast should still work after reload signal"
        );

        // Inbound receiver still has the message
        drop(core);
        let mut inbound = receivers.inbound;
        let msg = inbound.recv().await.unwrap();
        assert_eq!(msg.message.content, "hello");
    }

    #[test]
    fn backup_config_creates_bak_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "timezone = \"UTC\"\n").unwrap();

        backup_config(dir.path());

        let bak = dir.path().join("config.toml.bak");
        assert!(bak.exists(), "backup should create config.toml.bak");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "timezone = \"UTC\"\n",
            "backup content should match original"
        );
    }

    #[test]
    fn rollback_config_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let bak = dir.path().join("config.toml.bak");

        std::fs::write(&bak, "timezone = \"UTC\"\n").unwrap();
        std::fs::write(&config, "BROKEN").unwrap();

        assert!(rollback_config(dir.path()), "rollback should succeed");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "timezone = \"UTC\"\n",
        );
    }

    #[test]
    fn rollback_config_fails_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "BROKEN").unwrap();
        assert!(!rollback_config(dir.path()));
    }

    #[test]
    fn backup_config_missing_source_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // No config.toml exists — backup should warn but not panic
        backup_config(dir.path());
        assert!(
            !dir.path().join("config.toml.bak").exists(),
            "no backup should be created when source is missing"
        );
    }

    #[test]
    fn backup_config_overwrites_stale_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml.bak"), "old content").unwrap();
        std::fs::write(dir.path().join("config.toml"), "new content").unwrap();

        backup_config(dir.path());

        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml.bak")).unwrap(),
            "new content",
            "backup should overwrite previous backup"
        );
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
