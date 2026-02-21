//! WebSocket gateway server and main event loop.
//!
//! Accepts WebSocket connections from multiple clients and routes messages
//! through a single agent instance. All clients see responses and system
//! events; tool events are only forwarded to clients with verbose mode on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};

use crate::agent::Agent;
use crate::config::{Config, ModelSpec, ProviderKind, ProviderSpec};
use crate::cron::executor::execute_due_jobs;
use crate::cron::store::CronStore;
use crate::error::IronclawError;
use crate::memory::log_store::load_observation_log;
use crate::memory::observer::{Observer, ObserverConfig};
use crate::memory::recent_store::{
    append_recent_messages, clear_recent_messages, load_messages_for_agent, load_recent_messages,
};
use crate::memory::reflector::{Reflector, ReflectorConfig};
use crate::memory::search::create_shared_index;
use crate::memory::types::Visibility;
use crate::models::anthropic::AnthropicClient;
use crate::models::gemini::GeminiClient;
use crate::models::ollama::OllamaClient;
use crate::models::openai::OpenAiClient;
use crate::models::{CompletionOptions, HttpClientConfig, ModelProvider, SharedHttpClient};
use crate::pulse::executor::execute_pulse;
use crate::pulse::scheduler::PulseScheduler;
use crate::pulse::types::AlertLevel;
use crate::tools::ToolRegistry;
use crate::workspace::bootstrap::ensure_workspace;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::display::BroadcastDisplay;
use super::protocol::{ClientMessage, ServerMessage};

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
    /// Reload requested; caller should re-run with fresh config.
    Reload,
}

/// An inbound message from a WebSocket client.
struct InboundMessage {
    /// Client-generated correlation ID.
    id: String,
    /// The user message content.
    content: String,
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
struct GatewayState {
    inbound_tx: mpsc::Sender<InboundMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_sender: tokio::sync::watch::Sender<bool>,
    observe_notify: Arc<tokio::sync::Notify>,
    reflect_notify: Arc<tokio::sync::Notify>,
}

/// Start the WebSocket gateway server and run the main event loop.
///
/// This is the primary entrypoint for `ironclaw serve`. It:
/// 1. Bootstraps the workspace, model provider, tools, memory, cron, and pulse
/// 2. Spawns the axum WebSocket server
/// 3. Enters the main `tokio::select!` loop processing inbound messages,
///    pulse ticks, and cron ticks
///
/// # Errors
///
/// Returns `IronclawError` if initialization fails (config, workspace, provider,
/// search index, cron store) or if the WebSocket server cannot bind.
#[expect(
    clippy::too_many_lines,
    reason = "gateway entrypoint wires up all subsystems; splitting would obscure the startup sequence"
)]
pub async fn run_gateway(cfg: Config) -> Result<GatewayExit, IronclawError> {
    // Ensure workspace
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    ensure_workspace(&layout).await?;

    // Change to workspace directory
    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        IronclawError::Config(format!(
            "failed to change to workspace directory {}: {e}",
            cfg.workspace_dir.display()
        ))
    })?;
    tracing::info!(workspace = %cfg.workspace_dir.display(), "changed to workspace directory");

    // Load identity files
    let identity = IdentityFiles::load(&layout).await?;

    // Build shared HTTP client
    let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(cfg.timeout_secs))
        .map_err(|e| IronclawError::Config(format!("failed to build HTTP client: {e}")))?;

    // Build model provider
    let provider = build_provider_from_provider_spec(&cfg.main, cfg.max_tokens, http.clone())?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    // Build observer and reflector
    let (observer, reflector) = build_memory_components(&cfg, http.clone())?;

    // Build per-role providers for pulse and cron
    let pulse_provider =
        build_provider_from_provider_spec(&cfg.pulse, cfg.max_tokens, http.clone())?;
    let cron_provider = build_provider_from_provider_spec(&cfg.cron, cfg.max_tokens, http)?;

    // Build search index
    let search_index = create_shared_index(&layout.search_index_dir())?;
    match search_index.rebuild(&layout.memory_dir()) {
        Ok(count) => tracing::info!(indexed = count, "search index rebuilt"),
        Err(e) => eprintln!("warning: failed to rebuild search index: {e}"),
    }

    // Build cron store and notify
    let cron_store = Arc::new(tokio::sync::Mutex::new(
        CronStore::load(layout.cron_jobs_json()).await?,
    ));
    let cron_notify = Arc::new(tokio::sync::Notify::new());

    // Build tool registry
    let mut tools = ToolRegistry::new();
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker);
    tools.register_memory_tools(&layout);
    tools.register_search_tool(Arc::clone(&search_index));
    tools.register_cron_tools(Arc::clone(&cron_store), Arc::clone(&cron_notify));

    // Build completion options
    let options = CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
    };

    // Build agent
    let mut agent = Agent::new(provider, tools, identity, options);
    agent.reload_observations(&layout).await?;

    // Restore unobserved messages from previous session
    let recent_for_restore = load_messages_for_agent(&layout.recent_messages_json()).await?;
    if !recent_for_restore.is_empty() {
        tracing::info!(
            count = recent_for_restore.len(),
            "restoring recent messages from previous session"
        );
        agent.restore_messages(recent_for_restore);
    }

    // Channels
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(32);
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ServerMessage>(256);
    let (reload_sender, mut reload_rx) = tokio::sync::watch::channel(false);

    let broadcast_display = BroadcastDisplay::new(broadcast_tx.clone());

    let observe_notify = Arc::new(tokio::sync::Notify::new());
    let reflect_notify = Arc::new(tokio::sync::Notify::new());

    // Build axum app
    let state = GatewayState {
        inbound_tx,
        broadcast_tx: broadcast_tx.clone(),
        reload_sender,
        observe_notify: Arc::clone(&observe_notify),
        reflect_notify: Arc::clone(&reflect_notify),
    };

    let app = axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = cfg.gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| IronclawError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");

    // Spawn the HTTP server with graceful shutdown on reload signal
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

    // Pulse scheduler
    let mut pulse_scheduler = PulseScheduler::new();
    let pulse_enabled = cfg.pulse_enabled;

    // Timer intervals
    let mut pulse_tick = tokio::time::interval(tokio::time::Duration::from_secs(60));
    let mut cron_tick = tokio::time::interval(tokio::time::Duration::from_secs(30));

    // Skip first pulse tick
    pulse_tick.tick().await;

    tracing::info!("gateway ready, entering main loop");

    loop {
        tokio::select! {
            // ── Reload signal ─────────────────────────────────────────────
            _ = reload_rx.changed() => {
                tracing::info!("reloading configuration");
                server_handle.abort();
                return Ok(GatewayExit::Reload);
            }

            // ── Inbound WebSocket messages ────────────────────────────────
            msg = inbound_rx.recv() => {
                let Some(inbound) = msg else {
                    tracing::info!("inbound channel closed, shutting down");
                    server_handle.abort();
                    break;
                };

                let reply_id = inbound.id.clone();

                // Notify clients that we're processing this message
                if broadcast_tx.send(ServerMessage::TurnStarted {
                    reply_to: reply_id.clone(),
                }).is_err() {
                    tracing::trace!("no broadcast receivers for turn_started");
                }

                let before = agent.message_count();

                match agent.process_message(&inbound.content, &broadcast_display).await {
                    Ok(texts) => {
                        for text in texts {
                            if broadcast_tx.send(ServerMessage::Response {
                                reply_to: reply_id.clone(),
                                content: text,
                            }).is_err() {
                                tracing::trace!("no broadcast receivers for response");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "agent processing error");
                        if broadcast_tx.send(ServerMessage::Error {
                            reply_to: Some(reply_id),
                            message: e.to_string(),
                        }).is_err() {
                            tracing::trace!("no broadcast receivers for error");
                        }
                    }
                }

                let new_messages: Vec<_> = agent.messages_since(before).to_vec();
                let project_context = workspace_name(&layout);
                run_memory_pipeline(
                    &new_messages,
                    &project_context,
                    Visibility::User,
                    &observer,
                    &reflector,
                    &search_index,
                    &layout,
                    &mut agent,
                )
                .await;
            }

            // ── Pulse timer ───────────────────────────────────────────────
            _ = pulse_tick.tick(), if pulse_enabled => {
                let now = chrono::Utc::now();
                let due = pulse_scheduler.due_pulses(now, &layout.heartbeat_yml());

                for pulse in &due {
                    match execute_pulse(
                        pulse,
                        &agent,
                        &layout.alerts_md(),
                        Some(&*pulse_provider),
                    )
                    .await
                    {
                        Ok(result) => {
                            if !result.is_heartbeat_ok
                                && !matches!(result.alert_level, AlertLevel::Low)
                            {
                                let prefix = match result.alert_level {
                                    AlertLevel::High => format!("⚠ ALERT [{}]", result.pulse_name),
                                    AlertLevel::Medium | AlertLevel::Low => {
                                        format!("pulse: {}", result.pulse_name)
                                    }
                                };
                                if broadcast_tx.send(ServerMessage::SystemEvent {
                                    source: prefix,
                                    content: result.response.clone(),
                                }).is_err() {
                                    tracing::trace!("no broadcast receivers for pulse event");
                                }
                            }
                            let pulse_context = workspace_name(&layout);
                            run_memory_pipeline(
                                &result.messages,
                                &pulse_context,
                                Visibility::Background,
                                &observer,
                                &reflector,
                                &search_index,
                                &layout,
                                &mut agent,
                            )
                            .await;
                        }
                        Err(e) => {
                            tracing::warn!(pulse = %pulse.name, error = %e, "pulse failed");
                        }
                    }
                }
            }

            // ── Cron timer ────────────────────────────────────────────────
            _ = cron_tick.tick(), if cfg.cron_enabled => {
                run_due_cron_jobs_gateway(
                    &cron_store, &mut agent, &observer, &reflector,
                    &search_index, &layout, &broadcast_tx,
                    Some(&*cron_provider),
                ).await;
            }

            // ── Cron notify (tool mutation wakeup) ────────────────────────
            () = cron_notify.notified(), if cfg.cron_enabled => {
                run_due_cron_jobs_gateway(
                    &cron_store, &mut agent, &observer, &reflector,
                    &search_index, &layout, &broadcast_tx,
                    Some(&*cron_provider),
                ).await;
            }

            // ── Observe command wakeup ─────────────────────────────────────
            () = observe_notify.notified() => {
                run_forced_observe(
                    &observer,
                    &reflector,
                    &search_index,
                    &layout,
                    &mut agent,
                    &broadcast_tx,
                )
                .await;
            }

            // ── Reflect command wakeup ─────────────────────────────────────
            () = reflect_notify.notified() => {
                run_forced_reflect(&reflector, &layout, &mut agent, &broadcast_tx).await;
            }
        }
    }

    Ok(GatewayExit::Shutdown)
}

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

/// Handle a single WebSocket connection.
///
/// Splits the socket into read/write halves. A forwarding task reads from the
/// broadcast channel and sends events to the client (filtering tool events
/// based on the per-connection verbose flag). The read loop processes incoming
/// client messages.
async fn handle_connection(socket: WebSocket, state: GatewayState) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let verbose = Arc::new(AtomicBool::new(false));
    let verbose_fwd = Arc::clone(&verbose);

    let mut broadcast_rx = state.broadcast_tx.subscribe();

    // Forwarding task: broadcast → WebSocket client
    let fwd_handle = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            // Filter tool events for non-verbose clients
            if msg.is_verbose_only() && !verbose_fwd.load(Ordering::Relaxed) {
                continue;
            }

            let Ok(json) = serde_json::to_string(&msg) else {
                tracing::warn!("failed to serialize server message");
                continue;
            };

            if ws_tx.send(WsMessage::text(json)).await.is_err() {
                break; // client disconnected
            }
        }
    });

    // Read loop: WebSocket client → inbound channel
    while let Some(frame) = ws_rx.next().await {
        let raw = match frame {
            Ok(WsMessage::Text(txt)) => txt,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue, // ignore binary, ping, pong
            Err(e) => {
                tracing::debug!(error = %e, "websocket read error");
                break;
            }
        };

        let client_msg: ClientMessage = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(e) => {
                let err_msg = ServerMessage::Error {
                    reply_to: None,
                    message: format!("malformed message: {e}"),
                };
                // Send directly to this client, not broadcast
                if state.broadcast_tx.send(err_msg).is_err() {
                    tracing::trace!("no broadcast receivers for error");
                }
                continue;
            }
        };

        match client_msg {
            ClientMessage::SendMessage { id, content } => {
                let inbound = InboundMessage { id, content };
                if state.inbound_tx.send(inbound).await.is_err() {
                    tracing::warn!("inbound channel closed, dropping message");
                    break;
                }
            }
            ClientMessage::SetVerbose { enabled } => {
                verbose.store(enabled, Ordering::Relaxed);
                tracing::debug!(enabled, "client set verbose mode");
            }
            ClientMessage::Ping => {
                // Send pong through broadcast (all clients will filter; only this
                // one would care, but pong is cheap and non-verbose)
                if state.broadcast_tx.send(ServerMessage::Pong).is_err() {
                    tracing::trace!("no broadcast receivers for pong");
                }
            }
            ClientMessage::Reload => {
                tracing::info!("reload requested by client");
                // Notify all connected clients before the connection drops
                state.broadcast_tx.send(ServerMessage::Reloading).ok();
                // Signal the main loop and HTTP server
                state.reload_sender.send(true).ok();
            }
            ClientMessage::Observe => {
                tracing::info!("observe requested by client");
                state.observe_notify.notify_one();
            }
            ClientMessage::Reflect => {
                tracing::info!("reflect requested by client");
                state.reflect_notify.notify_one();
            }
        }
    }

    // Clean up: abort forwarding task when client disconnects
    fwd_handle.abort();
    tracing::debug!("client disconnected");
}

/// Execute due cron jobs, broadcast notifications, and run the memory pipeline.
#[expect(
    clippy::too_many_arguments,
    reason = "gateway cron helper requires all subsystem references; a struct would add indirection without clarity"
)]
async fn run_due_cron_jobs_gateway(
    cron_store: &Arc<tokio::sync::Mutex<CronStore>>,
    agent: &mut Agent,
    observer: &Observer,
    reflector: &Reflector,
    search_index: &crate::memory::search::MemoryIndex,
    layout: &WorkspaceLayout,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
    provider_override: Option<&dyn ModelProvider>,
) {
    let now = chrono::Utc::now();
    let mut store = cron_store.lock().await;

    // Reload from disk so external edits to jobs.json take effect immediately
    match CronStore::load(layout.cron_jobs_json()).await {
        Ok(fresh) => *store = fresh,
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload cron store from disk; using in-memory state");
        }
    }

    match execute_due_jobs(&mut store, agent, now, provider_override).await {
        Ok(result) => {
            for notif in &result.notifications {
                if broadcast_tx
                    .send(ServerMessage::SystemEvent {
                        source: format!("cron: {}", notif.job_name),
                        content: notif.text.clone(),
                    })
                    .is_err()
                {
                    tracing::trace!("no broadcast receivers for cron notification");
                }
            }
            if !result.messages.is_empty() {
                let cron_context = workspace_name(layout);
                run_memory_pipeline(
                    &result.messages,
                    &cron_context,
                    Visibility::Background,
                    observer,
                    reflector,
                    search_index,
                    layout,
                    agent,
                )
                .await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "cron execution failed");
        }
    }
}

/// Persist new messages and run the observer/reflector/search pipeline.
#[expect(
    clippy::too_many_arguments,
    reason = "gateway memory pipeline requires all subsystem references; a struct would add indirection without clarity"
)]
pub(crate) async fn run_memory_pipeline(
    new_messages: &[crate::models::Message],
    project_context: &str,
    visibility: Visibility,
    observer: &Observer,
    reflector: &Reflector,
    search_index: &crate::memory::search::MemoryIndex,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
) {
    if new_messages.is_empty() {
        return;
    }

    if let Err(e) = append_recent_messages(
        &layout.recent_messages_json(),
        new_messages,
        project_context,
        visibility,
    )
    .await
    {
        eprintln!("warning: failed to persist recent messages: {e}");
        return;
    }

    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: failed to load recent messages: {e}");
            return;
        }
    };

    if !observer.should_observe(&recent) {
        return;
    }

    match observer.observe(&recent, layout).await {
        Ok(result) => {
            tracing::info!(episode_id = %result.id, "observer extracted episode");

            if let Err(e) = clear_recent_messages(&layout.recent_messages_json()).await {
                eprintln!("warning: failed to clear recent messages: {e}");
            }
            agent.clear_session();

            match tokio::fs::read_to_string(&result.transcript_path).await {
                Ok(ep_content) => {
                    if let Err(e) = search_index
                        .index_file(&result.transcript_path.to_string_lossy(), &ep_content)
                    {
                        eprintln!("warning: failed to index episode: {e}");
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: failed to read episode file {}: {e}",
                        result.transcript_path.display()
                    );
                }
            }

            run_reflector_if_needed(reflector, layout).await;

            if let Err(e) = agent.reload_observations(layout).await {
                eprintln!("warning: failed to reload observations: {e}");
            }
        }
        Err(e) => {
            eprintln!("warning: observer failed: {e}");
        }
    }
}

/// Derive the workspace name from the root directory for use as project context.
fn workspace_name(layout: &WorkspaceLayout) -> String {
    layout
        .root()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Run the reflector if the observation log exceeds the threshold.
async fn run_reflector_if_needed(reflector: &Reflector, layout: &WorkspaceLayout) {
    let log = match load_observation_log(&layout.observations_json()).await {
        Ok(log) => log,
        Err(e) => {
            eprintln!("warning: failed to load observation log for reflection: {e}");
            return;
        }
    };

    if reflector.should_reflect(&log) {
        match reflector.reflect(layout).await {
            Ok(compressed) => {
                tracing::info!(
                    episodes = compressed.len(),
                    "reflector compressed observation log"
                );
            }
            Err(e) => {
                eprintln!("warning: reflector failed: {e}");
            }
        }
    }
}

/// Force an observation cycle regardless of token threshold.
///
/// Loads recent messages, runs the observer, clears recent messages, updates
/// the search index, optionally triggers reflection, and broadcasts a `Notice`.
async fn run_forced_observe(
    observer: &Observer,
    reflector: &Reflector,
    search_index: &Arc<crate::memory::search::MemoryIndex>,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
) {
    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: forced observe failed to load recent messages: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("observe failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
            return;
        }
    };

    if recent.is_empty() {
        if broadcast_tx
            .send(ServerMessage::Notice {
                message: "[memory] observe: no recent messages".to_string(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for notice");
        }
        return;
    }

    let result = match observer.observe(&recent, layout).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("warning: forced observe failed: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("observe failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
            return;
        }
    };

    if let Err(e) = clear_recent_messages(&layout.recent_messages_json()).await {
        eprintln!("warning: failed to clear recent messages after forced observe: {e}");
    }
    agent.clear_session();

    match tokio::fs::read_to_string(&result.transcript_path).await {
        Ok(ep_content) => {
            if let Err(e) =
                search_index.index_file(&result.transcript_path.to_string_lossy(), &ep_content)
            {
                eprintln!("warning: failed to index episode after forced observe: {e}");
            }
        }
        Err(e) => {
            eprintln!(
                "warning: failed to read episode file {}: {e}",
                result.transcript_path.display()
            );
        }
    }

    let reflected = match load_observation_log(&layout.observations_json()).await {
        Ok(log) if reflector.should_reflect(&log) => match reflector.reflect(layout).await {
            Ok(_) => true,
            Err(e) => {
                eprintln!("warning: reflector failed after forced observe: {e}");
                false
            }
        },
        Ok(_) => false,
        Err(e) => {
            eprintln!("warning: failed to load observation log for reflection check: {e}");
            false
        }
    };

    if let Err(e) = agent.reload_observations(layout).await {
        eprintln!("warning: failed to reload observations after forced observe: {e}");
    }

    let suffix = if reflected {
        "; reflection triggered"
    } else {
        ""
    };
    let notice = format!(
        "[memory] observed: {} ({} observations){suffix}",
        result.id, result.observation_count
    );
    if broadcast_tx
        .send(ServerMessage::Notice { message: notice })
        .is_err()
    {
        tracing::trace!("no broadcast receivers for notice");
    }
}

/// Force a reflection cycle regardless of observation log size.
///
/// Runs the reflector, reloads observations into the agent, and broadcasts a `Notice`.
async fn run_forced_reflect(
    reflector: &Reflector,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
) {
    match reflector.reflect(layout).await {
        Ok(compressed) => {
            if let Err(e) = agent.reload_observations(layout).await {
                eprintln!("warning: failed to reload observations after forced reflect: {e}");
            }
            if broadcast_tx
                .send(ServerMessage::Notice {
                    message: format!("[memory] reflected: {} observations", compressed.len()),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for notice");
            }
        }
        Err(e) => {
            eprintln!("warning: forced reflect failed: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("reflect failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
        }
    }
}

/// Build observer and reflector from fully-resolved provider specs on `Config`.
///
/// # Errors
/// Returns `IronclawError::Config` if either provider cannot be built.
pub(crate) fn build_memory_components(
    cfg: &Config,
    http: SharedHttpClient,
) -> Result<(Observer, Reflector), IronclawError> {
    let observer_provider =
        build_provider_from_provider_spec(&cfg.observer, cfg.max_tokens, http.clone())?;
    let reflector_provider =
        build_provider_from_provider_spec(&cfg.reflector, cfg.max_tokens, http)?;

    let observer = Observer::new(
        observer_provider,
        ObserverConfig {
            threshold_tokens: cfg.memory.observer_threshold_tokens,
        },
    );

    let reflector = Reflector::new(
        reflector_provider,
        ReflectorConfig {
            threshold_tokens: cfg.memory.reflector_threshold_tokens,
        },
    );

    Ok((observer, reflector))
}

/// Build a model provider from a resolved `ProviderSpec`.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
pub(crate) fn build_provider_from_provider_spec(
    spec: &ProviderSpec,
    max_tokens: u32,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    build_provider_from_spec(
        &spec.model,
        &spec.provider_url,
        spec.api_key.as_deref(),
        max_tokens,
        http,
    )
}

/// Build a model provider from explicit parameters.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
pub(crate) fn build_provider_from_spec(
    spec: &ModelSpec,
    url: &str,
    api_key: Option<&str>,
    max_tokens: u32,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    match spec.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                IronclawError::Config(
                    "anthropic requires an API key (set ANTHROPIC_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(AnthropicClient::new(
                http,
                url,
                key,
                &spec.model,
                max_tokens,
            )))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| {
                IronclawError::Config(
                    "gemini requires an API key (set GEMINI_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(GeminiClient::new(
                http,
                url,
                key,
                &spec.model,
                max_tokens,
            )))
        }
        ProviderKind::Ollama => Ok(Box::new(OllamaClient::with_http_client(
            http,
            url,
            &spec.model,
        ))),
        ProviderKind::OpenAi => {
            if let Some(key) = api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    url,
                    &spec.model,
                    key,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    url,
                    &spec.model,
                )))
            }
        }
    }
}
