//! WebSocket connection handler.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket};
use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::bus::EndpointName;
use crate::gateway::protocol::{ClientMessage, ServerMessage};
use crate::gateway::types::GatewayState;
use crate::inference::ImageData;
use crate::interfaces::types::MessageOrigin;
use crate::interfaces::websocket::subscriber::WsSubscribers;
use crate::workbench::forward::ArtifactsOrigin;
use crate::workspace::watch::{LIVE_UPDATES_OFF_MESSAGE, WatchHealth, WatchSet};

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
pub(super) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
    through_artifacts_origin: Option<Extension<ArtifactsOrigin>>,
) -> impl IntoResponse {
    // The upgraded connection runs in a task axum spawns with no span of its
    // own; carrying the request's keeps the agent's `agent` log field on it.
    let span = tracing::Span::current();
    // A workbench page's socket isn't the user looking at the chat, so it
    // doesn't count as a client (see `handle_connection`).
    let counts_as_client = through_artifacts_origin.is_none();
    ws.on_upgrade(move |socket| handle_connection(socket, state, counts_as_client).instrument(span))
}

/// Handle a single WebSocket connection.
///
/// Each connection subscribes to typed topics (`Endpoint` for responses, tool
/// activity, turn lifecycle, and intermediates; `Notification` for system
/// notices and errors) on the bus. A local channel
/// carries per-connection messages (pong, errors, inbox confirmations) that
/// bypass the bus. A forwarding task merges all sources and writes
/// `ServerMessage` frames to the WebSocket.
///
/// Verbose filtering is server-side: tool call and result events (the main
/// agent's and every session's) are dropped in the forwarding task when
/// verbose mode is off.
///
/// `counts_as_client` is false for a connection opened through the artifacts
/// origin: it neither resets the agent's unread count nor counts as someone
/// being connected.
async fn handle_connection(socket: WebSocket, state: GatewayState, counts_as_client: bool) {
    // While a client connection is open the agent's unread count stays at
    // zero: a client is there to show new messages.
    let _client = counts_as_client.then(|| state.activity.client_connected());
    let (mut ws_tx, mut ws_rx) = socket.split();

    // The workspace prefixes this connection watches: replaced by the read
    // loop, read by the forwarding task to filter change-feed batches.
    let (watch_set_tx, watch_set_rx) = tokio::sync::watch::channel(WatchSet::default());

    // Subscribe to typed bus topics for this connection
    let mut subs = match WsSubscribers::new(
        &state.bus_handle,
        &state.team_feed.bus,
        EndpointName::from("ws"),
        state.file_registry.clone(),
        watch_set_rx,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to bus topics for ws connection");
            return;
        }
    };

    // Local channel for per-connection messages (pong, errors, inbox responses)
    let (local_tx, mut local_rx) = mpsc::unbounded_channel::<ServerMessage>();

    // The read loop sees a client's Close frame, but the forwarding task
    // owns the send half that must answer it; this hands the frame across.
    let (close_tx, mut close_rx) = oneshot::channel::<Option<CloseFrame>>();

    // Per-connection verbose flag shared between read loop and forwarding task
    let verbose = Arc::new(AtomicBool::new(false));
    let verbose_fwd = Arc::clone(&verbose);

    // Forwarding task: bus subscribers + local channel → WebSocket client
    let fwd_handle = crate::util::spawn_in_span(async move {
        loop {
            let msg = tokio::select! {
                bus_msg = subs.recv() => {
                    match bus_msg {
                        Some(m) => Some(m),
                        None => break,
                    }
                }
                msg = local_rx.recv() => {
                    match msg {
                        Some(m) => Some(m),
                        None => break,
                    }
                }
                frame = &mut close_rx => {
                    // Answer the client's Close and shut the transport down:
                    // without both, the client sees code 1006 (abnormal
                    // closure) instead of its own.
                    if let Ok(frame) = frame {
                        ws_tx.send(WsMessage::Close(frame)).await.ok();
                    }
                    ws_tx.close().await.ok();
                    break;
                }
            };

            if let Some(msg) = msg {
                if !verbose_fwd.load(Ordering::Relaxed) && is_verbose_only(&msg) {
                    continue;
                }

                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to serialize server message");
                        continue;
                    }
                };
                if ws_tx.send(WsMessage::text(json)).await.is_err() {
                    break; // client disconnected
                }
            }
        }
    });

    // Read loop: WebSocket client → bus / local channel
    while let Some(frame) = ws_rx.next().await {
        let raw = match frame {
            Ok(WsMessage::Text(txt)) => txt,
            Ok(WsMessage::Close(frame)) => {
                answer_client_close(close_tx, fwd_handle, frame).await;
                tracing::debug!("client disconnected");
                return;
            }
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
                    details: None,
                };
                tracing::warn!(error = %e, "malformed WebSocket message from client");
                local_tx.send(err_msg).ok();
                continue;
            }
        };

        if !handle_client_message(client_msg, &state, &local_tx, &verbose, &watch_set_tx).await {
            break;
        }
    }

    // Clean up: abort forwarding task when client disconnects
    fwd_handle.abort();
    tracing::debug!("client disconnected");
}

/// Hand the client's Close frame to the forwarding task, which owns the send
/// half, and wait for it to answer before the connection tears down. Without
/// an answer the client sees code 1006 (abnormal closure) instead of its own.
async fn answer_client_close(
    close_tx: oneshot::Sender<Option<CloseFrame>>,
    fwd_handle: tokio::task::JoinHandle<()>,
    frame: Option<CloseFrame>,
) {
    if close_tx.send(frame).is_ok() {
        fwd_handle.await.ok();
    } else {
        fwd_handle.abort();
    }
}

/// Dispatch a single client message. Returns `false` to break the read loop.
#[expect(
    clippy::too_many_lines,
    reason = "match arms over many message variants"
)]
async fn handle_client_message(
    msg: ClientMessage,
    state: &GatewayState,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
    verbose: &AtomicBool,
    watch_set: &tokio::sync::watch::Sender<WatchSet>,
) -> bool {
    match msg {
        ClientMessage::SendMessage {
            id,
            content,
            images,
        } => {
            if !images.is_empty()
                && let Err(reason) = validate_images(&images)
            {
                local_tx
                    .send(ServerMessage::Error {
                        reply_to: Some(id),
                        message: reason,
                        details: None,
                    })
                    .ok();
                return true;
            }

            let origin = MessageOrigin {
                endpoint: "ws".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            };
            let msg_event = crate::bus::MessageEvent {
                id: id.clone(),
                content,
                origin,
                timestamp: crate::time::now_local(chrono_tz::UTC),
                images,
                context: None,
            };
            if let Err(e) = state
                .publisher
                .publish(crate::bus::topics::UserMessage, msg_event)
                .await
            {
                tracing::warn!(error = %e, "failed to publish message to bus");
                return false;
            }
        }
        ClientMessage::SetVerbose { enabled } => {
            verbose.store(enabled, Ordering::Relaxed);
        }
        ClientMessage::WatchWorkspace { prefixes } => {
            replace_watch_set(prefixes, watch_health(state), watch_set, local_tx);
        }
        ClientMessage::Ping => {
            local_tx.send(ServerMessage::Pong).ok();
        }
        ClientMessage::Reload => {
            tracing::info!("reload requested by client");
            local_tx.send(ServerMessage::Reloading).ok();
            state
                .reload_tx
                .send(crate::gateway::types::ReloadSignal::Agent)
                .ok();
        }
        ClientMessage::ServerCommand { name, args } => {
            tracing::info!(command = %name, "server command from client");
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            state
                .command_tx
                .send(crate::gateway::types::ServerCommand {
                    name,
                    args,
                    reply_tx: Some(reply_tx),
                })
                .await
                .ok();

            // Successful commands are already reflected via the normal bus
            // broadcast (e.g. Notice/InlineOutput); only a rejection needs
            // routing back here, scoped to this connection only.
            let err_tx = local_tx.clone();
            crate::util::spawn_in_span(async move {
                if let Ok(Err(reason)) = reply_rx.await {
                    err_tx
                        .send(ServerMessage::Error {
                            reply_to: None,
                            message: reason,
                            details: None,
                        })
                        .ok();
                }
            });
        }
        ClientMessage::Cancel { reply_to } => {
            tracing::info!(reply_to = %reply_to, "stop requested by client");
            if state
                .stop_tx
                .try_send(crate::gateway::types::StopRequest {
                    reply_to: Some(reply_to),
                    result_tx: None,
                })
                .is_err()
            {
                tracing::warn!("failed to dispatch stop request: channel closed or full");
            }
        }
        ClientMessage::SessionSendMessage {
            id,
            address,
            content,
        } => {
            tracing::info!(address = %address, "session message requested by client");
            handle_session_command(
                SessionCommand::SendMessage {
                    id,
                    address,
                    content,
                },
                state,
                local_tx,
            )
            .await;
        }
        ClientMessage::SessionStop { id, address } => {
            handle_session_command(SessionCommand::Stop { id, address }, state, local_tx).await;
        }
        ClientMessage::InboxAdd { body } => {
            tracing::info!("inbox add requested by client");
            let dir = state.agent_inbox_dir.clone();
            let tz = state.tz;
            let tx = local_tx.clone();
            crate::util::spawn_in_span(async move {
                let title = crate::inbox::derive_title(&body);
                match crate::inbox::quick_add(&dir, &title, &body, "cli", tz).await {
                    Ok(_filename) => {
                        tx.send(ServerMessage::Notice {
                            message: "Added a note to the inbox.".to_string(),
                        })
                        .ok();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "inbox add failed");
                        tx.send(ServerMessage::Error {
                            reply_to: None,
                            message: "Couldn't add a note to the inbox. Try again.".to_string(),
                            details: Some(format!("{e:#}")),
                        })
                        .ok();
                    }
                }
            });
        }
    }
    true
}

/// Whether live updates are off for either feed a connection can watch: the
/// agent's own directory or the hub's team directory.
fn watch_health(state: &GatewayState) -> WatchHealth {
    if *state.team_feed.health.borrow() == WatchHealth::Off {
        WatchHealth::Off
    } else {
        *state.workspace_watch_health.borrow()
    }
}

/// Apply a `watch_workspace` request: replace the connection's watch set, or
/// refuse the request with an `Error` frame and keep the current set. A
/// connection that starts watching while no watcher runs is told live
/// updates are off.
fn replace_watch_set(
    prefixes: Vec<String>,
    health: WatchHealth,
    watch_set: &tokio::sync::watch::Sender<WatchSet>,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
) {
    match WatchSet::parse(prefixes) {
        Ok(set) => {
            let watching = !set.is_empty();
            watch_set.send_replace(set);
            if watching && health == WatchHealth::Off {
                local_tx
                    .send(ServerMessage::WorkspaceWatchUnavailable {
                        message: LIVE_UPDATES_OFF_MESSAGE.to_string(),
                    })
                    .ok();
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "refused a workspace watch request");
            local_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("Couldn't watch the workspace: {e}."),
                    details: None,
                })
                .ok();
        }
    }
}

/// Whether `msg` is only sent to clients that turned verbose mode on: tool
/// call and result events, the main agent's and sessions' alike.
fn is_verbose_only(msg: &ServerMessage) -> bool {
    matches!(
        msg,
        ServerMessage::ToolCall { .. }
            | ServerMessage::ToolResult { .. }
            | ServerMessage::SessionToolCall { .. }
            | ServerMessage::SessionToolResult { .. }
    )
}

/// Carry out a sessions-sidebar command and reply to this connection only.
async fn handle_session_command(
    command: SessionCommand,
    state: &GatewayState,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
) {
    let reply = match command {
        SessionCommand::SendMessage {
            id,
            address,
            content,
        } => {
            match crate::gateway::sessions::send_session_message(
                &state.agent_messenger,
                &address,
                content,
                &crate::gateway::sessions::SessionMessageAuthor::Owner,
            )
            .await
            {
                Ok(outcome) => ServerMessage::SessionMessageDelivered {
                    id,
                    address,
                    outcome,
                },
                Err(e) => ServerMessage::SessionCommandFailed {
                    id,
                    address,
                    code: e.code,
                    message: e.message,
                },
            }
        }
        SessionCommand::Stop { id, address } => {
            match crate::gateway::sessions::stop_session(&state.session_registry, &address) {
                Ok(()) => ServerMessage::SessionStopRequested { id, address },
                Err(e) => ServerMessage::SessionCommandFailed {
                    id,
                    address,
                    code: e.code,
                    message: e.message,
                },
            }
        }
    };
    if local_tx.send(reply).is_err() {
        tracing::debug!("client disconnected before its session command reply was sent");
    }
}

/// The sessions-sidebar commands, split out of [`ClientMessage`] so
/// [`handle_session_command`] can own their handling.
enum SessionCommand {
    SendMessage {
        id: String,
        address: String,
        content: String,
    },
    Stop {
        id: String,
        address: String,
    },
}

/// Maximum raw image size in bytes, per the model API's per-image limit (5 MB).
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// MIME types the model API accepts for image input.
const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Validate image attachments before publishing to the bus.
///
/// Enforces the model API's own per-image limits — 5 MB and JPEG/PNG/GIF/WebP
/// only — since those are provider facts, not a residuum-imposed cap. There
/// is no limit on how many images a message can carry.
///
/// # Errors
///
/// Returns a human-readable error describing the first violation found.
fn validate_images(images: &[ImageData]) -> Result<(), String> {
    for img in images {
        if !ALLOWED_MIME_TYPES.contains(&img.media_type.as_str()) {
            return Err(format!(
                "unsupported image type: {} (allowed: {})",
                img.media_type,
                ALLOWED_MIME_TYPES.join(", ")
            ));
        }

        // Estimate raw bytes from base64 length: every 4 base64 chars = 3 bytes
        let estimated_bytes = img.data.len() * 3 / 4;
        if estimated_bytes > MAX_IMAGE_BYTES {
            return Err(format!(
                "image too large: ~{estimated_bytes} bytes (max {MAX_IMAGE_BYTES})"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_verbose_updates_flag() {
        let flag = AtomicBool::new(false);

        flag.store(true, Ordering::Relaxed);
        assert!(
            flag.load(Ordering::Relaxed),
            "should be true after storing true"
        );

        flag.store(false, Ordering::Relaxed);
        assert!(
            !flag.load(Ordering::Relaxed),
            "should be false after storing false"
        );
    }

    fn make_image(media_type: &str, data_len: usize) -> ImageData {
        ImageData {
            media_type: media_type.to_string(),
            data: "A".repeat(data_len),
        }
    }

    #[test]
    fn validate_images_accepts_valid_input() {
        let images = vec![make_image("image/jpeg", 100), make_image("image/png", 200)];
        assert!(
            validate_images(&images).is_ok(),
            "valid images should pass validation"
        );
    }

    #[test]
    fn validate_images_accepts_empty() {
        assert!(
            validate_images(&[]).is_ok(),
            "empty images should pass validation"
        );
    }

    #[test]
    fn validate_images_accepts_many_images() {
        // There is no cap on how many images a message can carry.
        let images: Vec<_> = (0..50).map(|_| make_image("image/png", 100)).collect();
        assert!(
            validate_images(&images).is_ok(),
            "any number of images should be accepted"
        );
    }

    #[test]
    fn validate_images_rejects_bad_mime_type() {
        let images = vec![make_image("image/bmp", 100)];
        let result = validate_images(&images);
        assert!(result.is_err(), "should reject unsupported MIME type");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("unsupported")),
            "error should mention 'unsupported'"
        );
    }

    #[test]
    fn validate_images_rejects_oversized() {
        // 7 MB worth of base64 (~9.3M base64 chars encode ~7M raw bytes)
        let oversized_len = 7 * 1024 * 1024 * 4 / 3;
        let images = vec![make_image("image/jpeg", oversized_len)];
        let result = validate_images(&images);
        assert!(result.is_err(), "should reject oversized image");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("too large")),
            "error should mention 'too large'"
        );
    }

    #[test]
    fn validate_images_allows_all_mime_types() {
        for mime in ALLOWED_MIME_TYPES {
            let images = vec![make_image(mime, 100)];
            assert!(validate_images(&images).is_ok(), "{mime} should be allowed");
        }
    }

    #[test]
    fn validate_images_accepts_max_bytes() {
        // Compute base64 length such that estimated_bytes == MAX_IMAGE_BYTES exactly.
        // estimated_bytes = data.len() * 3 / 4; we want this == MAX_IMAGE_BYTES (not greater).
        let max_b64_len = MAX_IMAGE_BYTES * 4 / 3 + 1;
        let images = vec![make_image("image/jpeg", max_b64_len)];
        assert!(
            validate_images(&images).is_ok(),
            "image at exactly MAX_IMAGE_BYTES should be accepted"
        );
    }

    #[test]
    fn validate_images_rejects_empty_media_type() {
        let images = vec![make_image("", 100)];
        let result = validate_images(&images);
        assert!(result.is_err(), "empty media_type should be rejected");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("unsupported")),
            "error should mention 'unsupported'"
        );
    }

    #[test]
    fn a_valid_watch_request_replaces_the_set() {
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        replace_watch_set(
            vec!["wiki".into()],
            WatchHealth::Native,
            &watch_tx,
            &local_tx,
        );
        assert!(watch_rx.borrow().matches("wiki/a.md"));
        assert!(
            local_rx.try_recv().is_err(),
            "a valid request needs no reply"
        );

        replace_watch_set(Vec::new(), WatchHealth::Native, &watch_tx, &local_tx);
        assert!(watch_rx.borrow().is_empty(), "[] turns watching off");
    }

    #[test]
    fn an_invalid_watch_prefix_yields_an_error_and_keeps_the_set() {
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        replace_watch_set(
            vec!["wiki".into()],
            WatchHealth::Native,
            &watch_tx,
            &local_tx,
        );
        replace_watch_set(
            vec!["notes".into(), "../secrets".into()],
            WatchHealth::Native,
            &watch_tx,
            &local_tx,
        );
        assert!(matches!(
            local_rx.try_recv(),
            Ok(ServerMessage::Error { reply_to: None, message, .. }) if message.contains("../secrets")
        ));
        assert!(watch_rx.borrow().matches("wiki/a.md"));
        assert!(!watch_rx.borrow().matches("notes/a.md"));
    }

    #[test]
    fn watching_while_the_watcher_is_off_says_live_updates_are_off() {
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        replace_watch_set(Vec::new(), WatchHealth::Off, &watch_tx, &local_tx);
        assert!(
            local_rx.try_recv().is_err(),
            "not watching, nothing to report"
        );
        replace_watch_set(vec![String::new()], WatchHealth::Off, &watch_tx, &local_tx);
        assert!(matches!(
            local_rx.try_recv(),
            Ok(ServerMessage::WorkspaceWatchUnavailable { .. })
        ));
    }

    #[test]
    fn verbose_filter_covers_main_and_session_tool_events_only() {
        let tool_call = ServerMessage::SessionToolCall {
            address: "spawned-a-0001".into(),
            run_id: "run-a".into(),
            id: "tc".into(),
            name: "exec".into(),
            arguments: serde_json::json!({}),
            server: None,
        };
        let tool_result = ServerMessage::SessionToolResult {
            address: "spawned-a-0001".into(),
            run_id: "run-a".into(),
            tool_call_id: "tc".into(),
            name: "exec".into(),
            output: "ok".into(),
            is_error: false,
        };
        let main_call = ServerMessage::ToolCall {
            id: "tc".into(),
            name: "exec".into(),
            arguments: serde_json::json!({}),
            server: None,
        };
        assert!(is_verbose_only(&tool_call));
        assert!(is_verbose_only(&tool_result));
        assert!(is_verbose_only(&main_call));

        let response = ServerMessage::SessionResponse {
            address: "spawned-a-0001".into(),
            run_id: "run-a".into(),
            turn_id: "run-a-t1".into(),
            content: "done".into(),
        };
        let error = ServerMessage::SessionError {
            address: "spawned-a-0001".into(),
            run_id: "run-a".into(),
            message: "loop limit".into(),
            details: None,
        };
        assert!(!is_verbose_only(&response));
        assert!(!is_verbose_only(&error));
    }

    /// A minimal but real `GatewayState`, for exercising
    /// `handle_client_message` directly rather than through the full
    /// WebSocket stack. Mirrors the helper in `gateway/web/inbox.rs`'s own
    /// test module.
    fn make_test_gateway_state(workspace_dir: &std::path::Path) -> GatewayState {
        let (core, _receivers) = crate::gateway::types::GatewayCore::new(
            workspace_dir.to_path_buf(),
            workspace_dir.to_path_buf(),
        );
        let session_registry =
            std::sync::Arc::new(crate::background::registry::SessionRegistry::new());
        let session_store = std::sync::Arc::new(crate::background::store::SessionStore::new(
            workspace_dir.join("sessions"),
        ));
        let agent_messenger =
            std::sync::Arc::new(crate::background::messaging::AgentMessenger::new(
                std::sync::Arc::clone(&session_registry),
                core.publisher.clone(),
                std::sync::Arc::clone(&session_store),
                crate::agent::hop::HopLimits { soft: 8, hard: 32 },
            ));

        GatewayState {
            reload_tx: core.reload_tx,
            command_tx: core.command_tx,
            stop_tx: core.stop_tx,
            agent_inbox_dir: workspace_dir.join("inbox/agent"),
            tz: chrono_tz::UTC,
            publisher: core.publisher,
            bus_handle: core.bus_handle,
            file_registry: crate::gateway::file_server::FileRegistry::new("scout"),
            webhooks: crate::interfaces::webhook::WebhookTable::default(),
            session_registry,
            session_store,
            agent_messenger,
            skill_state: crate::skills::SkillState::new_shared(
                crate::skills::SkillIndex::default(),
                vec![],
            ),
            workspace_watch_health: tokio::sync::watch::channel(
                crate::workspace::watch::WatchHealth::Native,
            )
            .1,
            team_feed: std::sync::Arc::new(crate::hub::services::TeamChangeFeed::idle()),
            action_store: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::actions::store::ActionStore::new_empty(
                    workspace_dir.join("scheduled_actions.json"),
                ),
            )),
            layout: crate::workspace::layout::WorkspaceLayout::new(workspace_dir),
            activity: crate::hub::activity::ActivityTracker::new(
                "test-agent",
                tokio::sync::broadcast::channel(4).0,
                crate::hub::agent_watch::AgentChangeFeed::new(),
            ),
        }
    }

    async fn recv_with_timeout(
        local_rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
    ) -> ServerMessage {
        tokio::time::timeout(std::time::Duration::from_millis(500), local_rx.recv())
            .await
            .expect("a message should have been sent")
            .expect("channel should still be open")
    }

    #[tokio::test]
    async fn inbox_add_success_notice_reads_as_plain_language() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        std::fs::create_dir_all(&state.agent_inbox_dir).unwrap();
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());

        let keep_going = handle_client_message(
            ClientMessage::InboxAdd {
                body: "remember this".to_string(),
            },
            &state,
            &local_tx,
            &verbose,
            &watch_tx,
        )
        .await;
        assert!(keep_going);

        let msg = recv_with_timeout(&mut local_rx).await;
        assert!(
            matches!(&msg, ServerMessage::Notice { message } if message == "Added a note to the inbox."),
            "expected a plain-language Notice, got {msg:?}"
        );
    }

    #[tokio::test]
    async fn inbox_add_failure_keeps_the_cause_out_of_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        // `agent_inbox_dir` is never created, so the save underneath
        // `quick_add` fails and the client should see a plain-language
        // notice with the cause moved to `details`.
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());

        let keep_going = handle_client_message(
            ClientMessage::InboxAdd {
                body: "remember this".to_string(),
            },
            &state,
            &local_tx,
            &verbose,
            &watch_tx,
        )
        .await;
        assert!(keep_going);

        let msg = recv_with_timeout(&mut local_rx).await;
        assert!(
            matches!(
                &msg,
                ServerMessage::Error { reply_to: None, message, details: Some(_) }
                    if message == "Couldn't add a note to the inbox. Try again."
            ),
            "expected a plain-language Error with the cause in details, got {msg:?}"
        );
    }
}
