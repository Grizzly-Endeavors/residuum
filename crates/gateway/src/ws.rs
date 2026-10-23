//! WebSocket connection handler.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket};
use axum::extract::{Extension, Query, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::types::GatewayState;
use crate::websocket::subscriber::WsSubscribers;
use residuum_bus::EndpointName;
use residuum_bus::origin::MessageOrigin;
use residuum_config::WEB_UI_ENDPOINT;
use residuum_hub::uploads::{StagedUpload, UploadError};
use residuum_inference::{ImageData, MessageAttachment};
use residuum_net::workbench::ArtifactsOrigin;
use residuum_protocol::{ClientMessage, ServerMessage};
use residuum_workspace::inbox::attachment::{
    AttachmentInfo, AttachmentItem, AttachmentKind, SavedAttachment, reserve_attachment_item,
};
use residuum_workspace::watch::{LIVE_UPDATES_OFF_MESSAGE, WatchHealth, WatchSet};

/// Query parameters of the agent `/ws` upgrade.
#[derive(serde::Deserialize)]
pub(super) struct SocketQuery {
    /// `true` for a socket that only follows files; any other value is ignored.
    watch_only: Option<String>,
}

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
pub(super) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
    through_artifacts_origin: Option<Extension<ArtifactsOrigin>>,
    Query(query): Query<SocketQuery>,
) -> impl IntoResponse {
    // The upgraded connection runs in a task axum spawns with no span of its
    // own; carrying the request's keeps the agent's `agent` log field on it.
    let span = tracing::Span::current();
    // A workbench page's socket, or one that only follows files, isn't the
    // user looking at the chat, so it doesn't count as a client (see
    // `handle_connection`).
    let watch_only = query.watch_only.as_deref() == Some("true");
    let counts_as_client = through_artifacts_origin.is_none() && !watch_only;
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
/// origin or with `watch_only=true`: it neither resets the agent's unread
/// count nor counts as someone being connected. Subscriptions and watches
/// work the same either way.
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
        EndpointName::from(WEB_UI_ENDPOINT),
        state.file_registry.clone(),
        watch_set_rx,
        &state.turn_journal,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to bus topics for ws connection");
            return;
        }
    };

    let resync = subs.resync_requests();
    // Local channel for per-connection messages (pong, errors, inbox responses)
    let (local_tx, mut local_rx) = mpsc::unbounded_channel::<ServerMessage>();

    // The read loop sees a client's Close frame, but the forwarding task
    // owns the send half that must answer it; this hands the frame across.
    let (close_tx, mut close_rx) = oneshot::channel::<Option<CloseFrame>>();

    // Per-connection verbose flag shared between read loop and forwarding task
    let verbose = Arc::new(AtomicBool::new(false));
    let verbose_fwd = Arc::clone(&verbose);

    // Forwarding task: bus subscribers + local channel → WebSocket client
    let fwd_handle = residuum_core::util::spawn_in_span(async move {
        loop {
            // Local frames first: a `message_refused` for a failed publish is
            // queued as the bus shuts down, and an unbiased pick could take the
            // ended bus subscription and stop before writing it.
            let msg = tokio::select! {
                biased;
                msg = local_rx.recv() => {
                    match msg {
                        Some(m) => Some(m),
                        None => break,
                    }
                }
                bus_msg = subs.recv() => {
                    match bus_msg {
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

            let Some(msg) = msg.and_then(|m| shown_frame(m, verbose_fwd.load(Ordering::Relaxed)))
            else {
                continue;
            };
            if !send_frame(&mut ws_tx, &msg).await {
                break; // client disconnected
            }
        }
        // Nothing more will be forwarded: close, so the client knows to reconnect.
        ws_tx.close().await.ok();
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

        let channels = ConnectionChannels {
            local: &local_tx,
            verbose: &verbose,
            watch_set: &watch_set_tx,
            resync: &resync,
        };
        if !handle_client_message(client_msg, &state, &channels).await {
            break;
        }
    }

    // Dropping the senders lets the forwarder write what is already queued
    // (above all the `message_refused` for a failed publish, which ends this
    // loop) and then close the socket. Aborting it outright lost that frame,
    // so the client saw a bare close and its message vanished unrefused.
    drop(local_tx);
    drop(close_tx);
    finish_forwarding(fwd_handle).await;
    tracing::debug!("client disconnected");
}

/// How long the forwarder gets to write its queued frames once the read loop
/// has ended, before it is aborted.
const FORWARDER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Wait for the forwarding task to flush its queued frames and close the
/// socket, aborting it if a stuck socket keeps it past
/// [`FORWARDER_DRAIN_TIMEOUT`].
async fn finish_forwarding(mut fwd_handle: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(FORWARDER_DRAIN_TIMEOUT, &mut fwd_handle)
        .await
        .is_err()
    {
        tracing::warn!(
            timeout = ?FORWARDER_DRAIN_TIMEOUT,
            "websocket forwarder did not flush its queued frames in time; aborting it"
        );
        fwd_handle.abort();
    }
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

/// The per-connection state a client message can act on.
struct ConnectionChannels<'a> {
    /// Replies to this connection only.
    local: &'a mpsc::UnboundedSender<ServerMessage>,
    /// Whether tool events reach this connection.
    verbose: &'a AtomicBool,
    /// The workspace prefixes this connection watches.
    watch_set: &'a tokio::sync::watch::Sender<WatchSet>,
    /// Asks the forwarding task for a `turn_snapshot` frame.
    resync: &'a mpsc::UnboundedSender<()>,
}

/// Dispatch a single client message. Returns `false` to break the read loop.
#[expect(
    clippy::too_many_lines,
    reason = "match arms over many message variants"
)]
async fn handle_client_message(
    msg: ClientMessage,
    state: &GatewayState,
    channels: &ConnectionChannels<'_>,
) -> bool {
    let ConnectionChannels {
        local: local_tx,
        verbose,
        watch_set,
        resync,
    } = *channels;
    match msg {
        ClientMessage::SendMessage {
            id,
            content,
            attachments,
        } => {
            // The id names the turn this message starts or joins in every
            // frame about it, and an empty one reads as a message posted
            // outside any turn. A client that sent none gets one made here.
            let id = if id.is_empty() {
                let generated = format!("ws-{}", uuid::Uuid::new_v4());
                tracing::warn!(
                    message_id = %generated,
                    "client sent a message with an empty id; gave it one"
                );
                generated
            } else {
                id
            };
            let attached = match attach_uploads(state, content, &attachments).await {
                Ok(attached) => attached,
                Err(message) => {
                    local_tx
                        .send(ServerMessage::MessageRefused { id, message })
                        .ok();
                    return true;
                }
            };

            let origin = MessageOrigin {
                endpoint: WEB_UI_ENDPOINT.to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            };
            let companion_item_ids: Vec<String> = attached
                .attachments
                .iter()
                .map(|attachment| attachment.item_id.clone())
                .collect();
            let msg_event = residuum_bus::MessageEvent {
                id: id.clone(),
                content: attached.content,
                origin,
                timestamp: residuum_core::time::now_local(chrono_tz::UTC),
                images: attached.images,
                attachments: attached.attachments,
                context: None,
            };
            if let Err(e) = state
                .publisher
                .publish(residuum_bus::topics::UserMessage, msg_event)
                .await
            {
                tracing::warn!(error = %e, "failed to publish message to bus");
                discard_companion_items(state, &companion_item_ids).await;
                local_tx
                    .send(ServerMessage::MessageRefused {
                        id,
                        message: "Your message didn't reach the agent. Send it again.".to_string(),
                    })
                    .ok();
                return false;
            }
            consume_uploads(state, &attached.upload_ids).await;
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
        ClientMessage::ResyncTurn => {
            resync.send(()).ok();
        }
        ClientMessage::Reload => {
            tracing::info!("reload requested by client");
            local_tx.send(ServerMessage::Reloading).ok();
            state
                .reload_tx
                .send(residuum_bus::control::ReloadSignal::Agent)
                .ok();
        }
        ClientMessage::ServerCommand { name, args } => {
            tracing::info!(command = %name, "server command from client");
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            state
                .command_tx
                .send(residuum_bus::control::ServerCommand {
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
            residuum_core::util::spawn_in_span(async move {
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
                .try_send(residuum_bus::control::StopRequest {
                    reply_to: Some(reply_to),
                    result_tx: None,
                })
                .is_err()
            {
                tracing::warn!("failed to dispatch stop request: channel closed or full");
            }
        }
        ClientMessage::SendQueuedNow { id } => {
            send_queued_now(state, id, local_tx);
        }
        ClientMessage::WithdrawQueued { id } => {
            withdraw_queued(state, id, local_tx).await;
        }
        ClientMessage::SessionSendMessage {
            id,
            address,
            content,
            message_id,
        } => {
            tracing::info!(address = %address, "session message requested by client");
            handle_session_command(
                SessionCommand::SendMessage {
                    id,
                    address,
                    content,
                    message_id,
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
            // A note is mail from the user to this agent, so it shows in Sent.
            let mailbox =
                residuum_workspace::inbox::mail::Mailbox::for_workspace(&state.layout, state.tz);
            let tx = local_tx.clone();
            residuum_core::util::spawn_in_span(async move {
                let note = residuum_workspace::inbox::mail::OutgoingMail {
                    to: vec![mailbox.agent.clone()],
                    title: String::new(),
                    body,
                    source: residuum_workspace::inbox::mail::UserMailSource::Cli,
                    in_reply_to: None,
                    attachments: Vec::new(),
                };
                match residuum_workspace::inbox::mail::send_mail(&mailbox, note).await {
                    Ok(_sent) => {
                        tx.send(ServerMessage::Notice {
                            message: "Added a note to the inbox.".to_string(),
                            reload: None,
                            reload_of: None,
                        })
                        .ok();
                    }
                    Err(residuum_workspace::inbox::mail::SendMailError::BlankBody) => {
                        tx.send(ServerMessage::Error {
                            reply_to: None,
                            message: "A note needs some text.".to_string(),
                            details: None,
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
/// Write one frame to the client. Returns `false` once the client is gone; a
/// frame that can't be serialized is logged and skipped.
async fn send_frame(
    ws_tx: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
    msg: &ServerMessage,
) -> bool {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize server message");
            return true;
        }
    };
    ws_tx.send(WsMessage::text(json)).await.is_ok()
}

/// Send a waiting message now: it is held out of the running turn, which is
/// stopped, so the message starts the next one. Whichever turn is running is
/// stopped, since a message left waiting after its own turn ended waits for
/// the turn now running. A message no longer waiting is answered to this
/// connection only.
fn send_queued_now(
    state: &GatewayState,
    id: String,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
) {
    if !state.queued.send_now(&id) {
        tracing::info!(msg_id = %id, "send now asked for a message no longer waiting");
        local_tx
            .send(ServerMessage::QueuedMessageUnavailable { id })
            .ok();
        return;
    }
    tracing::info!(msg_id = %id, "sending a waiting message now: stopping the running turn");
    if state
        .stop_tx
        .try_send(residuum_bus::control::StopRequest {
            reply_to: None,
            result_tx: None,
        })
        .is_err()
    {
        // The message still starts the next turn, ahead of anything else
        // waiting; only the stop that would have brought that turn sooner is lost.
        tracing::warn!(msg_id = %id, "failed to dispatch the stop for a message to send now: channel closed or full");
    }
}

/// Take back a waiting message, so the agent never reads it. Every
/// connection hears it was withdrawn; a message no longer waiting is answered
/// to this connection only.
async fn withdraw_queued(
    state: &GatewayState,
    id: String,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
) {
    if !state.queued.withdraw(&id) {
        tracing::info!(msg_id = %id, "withdraw asked for a message no longer waiting");
        local_tx
            .send(ServerMessage::QueuedMessageUnavailable { id })
            .ok();
        return;
    }
    tracing::info!(msg_id = %id, "withdrew a waiting message");
    if let Err(e) = state
        .publisher
        .publish(
            residuum_bus::topics::MainConversation,
            residuum_bus::MainConversationEvent::MessageWithdrawn { id: id.clone() },
        )
        .await
    {
        // It is withdrawn either way; only other pages keep showing it as
        // waiting, until their next snapshot or reload.
        tracing::warn!(error = %e, msg_id = %id, "failed to announce a withdrawn message");
        local_tx.send(ServerMessage::MessageWithdrawn { id }).ok();
    }
}

/// `msg` as a connection gets it: a connection that isn't verbose gets no
/// tool frames, neither on their own nor inside a `turn_snapshot`.
fn shown_frame(mut msg: ServerMessage, verbose: bool) -> Option<ServerMessage> {
    if verbose {
        return Some(msg);
    }
    if is_verbose_only(&msg) {
        return None;
    }
    if let ServerMessage::TurnSnapshot {
        turn: Some(turn), ..
    } = &mut msg
    {
        turn.frames.retain(|entry| !is_verbose_only(&entry.frame));
    }
    Some(msg)
}

fn is_verbose_only(msg: &ServerMessage) -> bool {
    matches!(
        msg,
        ServerMessage::ToolCall { .. }
            | ServerMessage::ToolResult { .. }
            | ServerMessage::SessionToolCall { .. }
            | ServerMessage::SessionToolResult { .. }
    )
}

/// Carry out a session command from the web UI and reply to this connection only.
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
            message_id,
        } => {
            match crate::sessions::send_session_message(
                &state.agent_messenger,
                &address,
                message_id,
                content,
                &crate::sessions::SessionMessageAuthor::Owner,
            )
            .await
            {
                Ok(sent) => ServerMessage::SessionMessageDelivered {
                    id,
                    address,
                    message_id: sent.id,
                    outcome: sent.outcome,
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
            match crate::sessions::stop_session(&state.session_registry, &address) {
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

/// The session commands the web UI sends, split out of [`ClientMessage`] so
/// [`handle_session_command`] can own their handling.
enum SessionCommand {
    SendMessage {
        id: String,
        address: String,
        content: String,
        message_id: Option<String>,
    },
    Stop {
        id: String,
        address: String,
    },
}

/// The platform a web chat attachment's companion item is filed under.
const CHAT_PLATFORM: &str = "Chat";

/// The author line of a web chat attachment's companion item.
const CHAT_AUTHOR: &str = "Web chat";

/// What a chat message says when one of its uploads is no longer staged.
const UPLOAD_UNAVAILABLE_MESSAGE: &str =
    "An attached file wasn't available anymore, so it's uploading again. Send once it's ready.";

/// A chat message once its uploads are saved into the agent's inbox.
struct AttachedUploads {
    /// The message text, with an `[Attachment: …]` line for each file.
    content: String,
    /// Supported images among the files, inlined for the model.
    images: Vec<ImageData>,
    /// The files, as the message carries them to history and the web.
    attachments: Vec<MessageAttachment>,
    /// The staged uploads the message used, removed once it is published.
    upload_ids: Vec<String>,
}

/// An upload whose file is copied into its companion item.
struct CopiedUpload {
    item: AttachmentItem,
    info: AttachmentInfo,
    saved: SavedAttachment,
}

/// Turn a chat message's upload ids into the companion inbox items they
/// stand for, the same attachments a Discord message carries.
///
/// All or nothing: every id is resolved before anything is copied, and a
/// failed copy discards the items already made, so a refused message leaves
/// no inbox item behind. Returns the plain-language message to send back when
/// the message is refused. Repeated ids count once.
async fn attach_uploads(
    state: &GatewayState,
    mut content: String,
    upload_ids: &[String],
) -> Result<AttachedUploads, String> {
    let staged = resolve_uploads(state, upload_ids).await?;
    let copied = save_uploads(state, &staged).await?;

    let mut images = Vec::new();
    let mut attachments = Vec::with_capacity(copied.len());
    for upload in copied {
        let finalized = upload
            .item
            .finalize(&upload.saved, &upload.info, &mut content, CHAT_AUTHOR)
            .await;
        images.extend(finalized.image);
        attachments.push(finalized.attachment);
    }

    Ok(AttachedUploads {
        content,
        images,
        attachments,
        upload_ids: staged.into_iter().map(|upload| upload.id).collect(),
    })
}

/// Resolve each distinct upload id to its staged file, refusing the whole
/// message on the first id that is not available.
async fn resolve_uploads(
    state: &GatewayState,
    upload_ids: &[String],
) -> Result<Vec<StagedUpload>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut staged = Vec::with_capacity(upload_ids.len());
    for id in upload_ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        match state.uploads.resolve(id).await {
            Ok(upload) => staged.push(upload),
            Err(e @ UploadError::Io { .. }) => {
                tracing::error!(upload_id = %id, error = %e, "failed to read a chat attachment");
                return Err(UPLOAD_UNAVAILABLE_MESSAGE.to_string());
            }
            Err(e) => {
                tracing::warn!(
                    upload_id = %id,
                    error = %e,
                    "chat message named an attachment that isn't staged"
                );
                return Err(UPLOAD_UNAVAILABLE_MESSAGE.to_string());
            }
        }
    }
    Ok(staged)
}

/// Copy every staged file into its own companion item. If one copy fails,
/// the items already copied are discarded and the message is refused.
async fn save_uploads(
    state: &GatewayState,
    staged: &[StagedUpload],
) -> Result<Vec<CopiedUpload>, String> {
    let mut copied = Vec::with_capacity(staged.len());
    for upload in staged {
        match save_upload(state, upload).await {
            Ok(done) => copied.push(done),
            Err(reason) => {
                tracing::error!(
                    upload_id = %upload.id,
                    filename = %upload.filename,
                    error = %reason,
                    "failed to copy a chat attachment into the agent inbox"
                );
                for done in copied {
                    done.item.discard().await;
                }
                return Err(format!(
                    "Couldn't attach {}. Try sending it again.",
                    upload.filename
                ));
            }
        }
    }
    Ok(copied)
}

/// Reserve a companion item for one staged file and copy the file into it.
/// A failed copy discards the item it reserved.
async fn save_upload(state: &GatewayState, upload: &StagedUpload) -> Result<CopiedUpload, String> {
    let info = AttachmentInfo {
        filename: upload.filename.clone(),
        size: u32::try_from(upload.size)
            .map_err(|_too_large| format!("file is {} bytes, too large to attach", upload.size))?,
        content_type: Some(upload.content_type.clone()),
    };
    let item = reserve_attachment_item(
        &state.agent_inbox_dir,
        &info,
        CHAT_PLATFORM,
        AttachmentKind::Attachment,
        state.tz,
    )
    .await?;
    match item.save_file(&info, &upload.path).await {
        Ok(saved) => Ok(CopiedUpload { item, info, saved }),
        Err(reason) => {
            item.discard().await;
            Err(reason)
        }
    }
}

/// Remove the companion items of a message that did not reach the bus, so a
/// resend does not file each attachment a second time. The staged uploads stay
/// for that resend. A failed removal is logged, and the item stays in the inbox.
async fn discard_companion_items(state: &GatewayState, item_ids: &[String]) {
    for id in item_ids {
        if let Err(e) = residuum_workspace::inbox::remove_item(&state.agent_inbox_dir, id).await {
            tracing::warn!(
                item_id = %id,
                error = %e,
                "failed to remove a companion item of a chat message that did not publish"
            );
        }
    }
}

/// Remove the staged uploads a published message used. A failed removal is
/// logged; the store's expiry clears the file later.
async fn consume_uploads(state: &GatewayState, upload_ids: &[String]) {
    for id in upload_ids {
        if let Err(e) = state.uploads.remove(id).await {
            tracing::warn!(
                upload_id = %id,
                error = %e,
                "failed to remove a sent chat attachment; it expires later"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_core::testing::wait;
    use tokio_tungstenite::tungstenite::Message as WireMessage;

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
            auto_mode: None,
        };
        let main_call = ServerMessage::ToolCall {
            reply_to: "turn-1".into(),
            call: 0,
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
        make_test_gateway_state_with_receivers(workspace_dir).0
    }

    /// [`make_test_gateway_state`], with the receivers the event loop would
    /// read the state's channels from.
    fn make_test_gateway_state_with_receivers(
        workspace_dir: &std::path::Path,
    ) -> (GatewayState, crate::types::CoreReceivers) {
        let (core, receivers) = crate::types::GatewayCore::new(
            workspace_dir.to_path_buf(),
            workspace_dir.to_path_buf(),
        );
        let session_registry =
            std::sync::Arc::new(residuum_background::registry::SessionRegistry::new());
        let session_store = std::sync::Arc::new(residuum_background::store::SessionStore::new(
            workspace_dir.join("sessions"),
        ));
        let agent_messenger =
            std::sync::Arc::new(residuum_background::messaging::AgentMessenger::new(
                std::sync::Arc::clone(&session_registry),
                core.publisher.clone(),
                std::sync::Arc::clone(&session_store),
                residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
            ));

        let state = GatewayState {
            turn_journal: crate::turn_journal::TurnJournal::default(),
            pulse_fires: std::sync::Arc::default(),
            queued: std::sync::Arc::default(),
            reload_tx: core.reload_tx,
            command_tx: core.command_tx,
            stop_tx: core.stop_tx,
            agent_inbox_dir: workspace_dir.join("inbox/agent"),
            uploads: residuum_hub::uploads::UploadStore::open(&workspace_dir.join("hub")).unwrap(),
            tz: chrono_tz::UTC,
            publisher: core.publisher,
            bus_handle: core.bus_handle,
            file_registry: residuum_hub::file_server::FileRegistry::new("scout"),
            webhooks: residuum_interfaces::webhook::WebhookTable::default(),
            session_registry,
            session_store,
            agent_messenger,
            skill_state: residuum_skills::SkillState::new_shared(
                residuum_skills::SkillIndex::default(),
                vec![],
            ),
            workspace_watch_health: tokio::sync::watch::channel(
                residuum_workspace::watch::WatchHealth::Native,
            )
            .1,
            team_feed: std::sync::Arc::new(residuum_hub::services::TeamChangeFeed::idle()),
            action_store: std::sync::Arc::new(tokio::sync::Mutex::new(
                residuum_proactivity::actions::store::ActionStore::new_empty(
                    workspace_dir.join("scheduled_actions.json"),
                ),
            )),
            layout: residuum_config::WorkspaceLayout::new(workspace_dir),
            activity: residuum_hub::activity::ActivityTracker::new(
                "test-agent",
                tokio::sync::broadcast::channel(4).0,
                residuum_hub::agent_watch::AgentChangeFeed::new(),
            ),
        };
        (state, receivers)
    }

    /// Serve [`ws_handler`] for `state` on a loopback port.
    async fn serve_ws_handler(state: GatewayState) -> std::net::SocketAddr {
        let app = axum::Router::new()
            .route("/ws", axum::routing::get(ws_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        addr
    }

    /// Open a socket to `path` and return it once its first `pong` arrives.
    /// The connection sends that pong from its read loop, which runs after
    /// the handler has done its activity accounting, so the counts can be
    /// read as soon as this returns.
    async fn open_socket_past_handshake(
        addr: std::net::SocketAddr,
        path: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let (mut socket, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
                .await
                .unwrap();
        let ping = serde_json::to_string(&ClientMessage::Ping).unwrap();
        socket.send(WireMessage::text(ping)).await.unwrap();
        let reply = wait::guarded("a pong on the socket", socket.next())
            .await
            .expect("socket closed")
            .expect("socket error");
        let expected = serde_json::to_string(&ServerMessage::Pong).unwrap();
        match reply {
            WireMessage::Text(text) => assert_eq!(text.as_str(), expected),
            other @ (WireMessage::Binary(_)
            | WireMessage::Ping(_)
            | WireMessage::Pong(_)
            | WireMessage::Close(_)
            | WireMessage::Frame(_)) => panic!("expected a text pong, got {other:?}"),
        }
        socket
    }

    /// How many clients the agent counts as connected once a socket opened
    /// on `path` has answered a ping.
    async fn connected_clients_after_socket_on(path: &str) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let activity = Arc::clone(&state.activity);
        let addr = serve_ws_handler(state).await;
        let _socket = open_socket_past_handshake(addr, path).await;
        activity.connected_clients()
    }

    #[tokio::test]
    async fn a_watch_only_socket_does_not_count_as_a_connected_client() {
        assert_eq!(
            connected_clients_after_socket_on("/ws?watch_only=true").await,
            0
        );
    }

    #[tokio::test]
    async fn a_socket_without_watch_only_counts_as_a_connected_client() {
        assert_eq!(connected_clients_after_socket_on("/ws").await, 1);
    }

    #[tokio::test]
    async fn any_other_watch_only_value_counts_as_a_connected_client() {
        assert_eq!(
            connected_clients_after_socket_on("/ws?watch_only=false").await,
            1
        );
        assert_eq!(
            connected_clients_after_socket_on("/ws?watch_only=yes").await,
            1
        );
    }

    async fn next_server_message(
        local_rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
    ) -> ServerMessage {
        wait::next("a message to be sent", local_rx).await
    }

    /// Handle one client message on a connection with no watches, returning
    /// what was sent back to that connection alone.
    async fn handle_on_connection(
        msg: ClientMessage,
        state: &GatewayState,
    ) -> mpsc::UnboundedReceiver<ServerMessage> {
        let (keep_going, local_rx) = handle_on_connection_with_outcome(msg, state).await;
        assert!(keep_going, "the connection should stay open");
        local_rx
    }

    /// [`handle_on_connection`], also returning whether the connection's read
    /// loop goes on.
    async fn handle_on_connection_with_outcome(
        msg: ClientMessage,
        state: &GatewayState,
    ) -> (bool, mpsc::UnboundedReceiver<ServerMessage>) {
        let (local_tx, local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let keep_going = handle_client_message(
            msg,
            state,
            &ConnectionChannels {
                local: &local_tx,
                verbose: &verbose,
                watch_set: &watch_tx,
                resync: &mpsc::unbounded_channel().0,
            },
        )
        .await;
        (keep_going, local_rx)
    }

    #[tokio::test]
    async fn withdrawing_a_waiting_message_tells_every_connection() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let mut main: residuum_bus::Subscriber<residuum_bus::MainConversationEvent> = state
            .bus_handle
            .subscribe(residuum_bus::topics::MainConversation)
            .await
            .unwrap();
        state.queued.enqueue("m1");

        let mut local =
            handle_on_connection(ClientMessage::WithdrawQueued { id: "m1".into() }, &state).await;

        let event = main.recv().await.unwrap().unwrap();
        assert!(
            matches!(&event, residuum_bus::MainConversationEvent::MessageWithdrawn { id } if id == "m1"),
            "expected the withdrawal on the main conversation, got {event:?}"
        );
        assert!(
            local.try_recv().is_err(),
            "nothing for this connection alone"
        );
        assert!(!state.queued.send_now("m1"), "it no longer waits");
    }

    #[tokio::test]
    async fn acting_on_a_message_no_longer_waiting_says_so_to_that_connection() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        for msg in [
            ClientMessage::WithdrawQueued { id: "read".into() },
            ClientMessage::SendQueuedNow { id: "read".into() },
        ] {
            let mut local = handle_on_connection(msg, &state).await;
            let reply = next_server_message(&mut local).await;
            assert!(
                matches!(&reply, ServerMessage::QueuedMessageUnavailable { id } if id == "read"),
                "expected QueuedMessageUnavailable, got {reply:?}"
            );
        }
    }

    #[tokio::test]
    async fn sending_a_waiting_message_now_stops_whichever_turn_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mut receivers) = make_test_gateway_state_with_receivers(dir.path());
        state.queued.enqueue("m1");

        let mut local =
            handle_on_connection(ClientMessage::SendQueuedNow { id: "m1".into() }, &state).await;

        let stop = receivers.stop.try_recv().expect("a stop request");
        assert_eq!(stop.reply_to, None);
        assert!(state.queued.is_send_now("m1"));
        assert!(
            local.try_recv().is_err(),
            "nothing for this connection alone"
        );
    }

    /// Stage a file in the hub's upload store, returning its upload id.
    async fn stage_upload(state: &GatewayState, filename: &str, bytes: &[u8]) -> String {
        state.uploads.stage(filename, bytes).await.unwrap().id
    }

    /// Subscribe to the user messages the bus publishes.
    async fn user_messages(
        state: &GatewayState,
    ) -> residuum_bus::Subscriber<residuum_bus::MessageEvent> {
        state
            .bus_handle
            .subscribe(residuum_bus::topics::UserMessage)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_chat_upload_becomes_a_companion_item_and_an_inline_image() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let photo_id = stage_upload(&state, "photo.png", b"\x89PNG fake pixels").await;
        let mut inbound = user_messages(&state).await;

        // The same id twice counts once.
        let _local = handle_on_connection(
            ClientMessage::SendMessage {
                id: "chat-1".into(),
                content: "look".into(),
                attachments: vec![photo_id.clone(), photo_id.clone()],
            },
            &state,
        )
        .await;

        let event = inbound.recv().await.unwrap().unwrap();
        let attachment_line = event
            .content
            .strip_prefix("look\n")
            .expect("the text is followed by the attachment line");
        assert!(
            attachment_line.starts_with("[Attachment: photo.png (")
                && attachment_line.ends_with(']'),
            "expected an attachment line, got {attachment_line:?}"
        );

        assert_eq!(event.images.len(), 1, "a PNG is inlined for the model");
        assert_eq!(
            event.images.first().map(|image| image.media_type.as_str()),
            Some("image/png")
        );

        assert_eq!(event.attachments.len(), 1, "a repeated id attaches once");
        let attachment = event.attachments.first().expect("one attachment");
        assert_eq!(attachment.filename, "photo.png");

        let items = residuum_workspace::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items.len(), 1, "one companion item in the agent inbox");
        let (file_name, item) = items.first().expect("one companion item");
        assert_eq!(
            file_name.as_str(),
            attachment.item_id.as_str(),
            "the attachment names the companion item"
        );
        assert_eq!(item.source, "chat");
        assert_eq!(item.title, "Chat attachment: photo.png");
        assert_eq!(item.body.lines().next(), Some("From: Web chat"));

        assert!(
            state.uploads.resolve(&photo_id).await.is_err(),
            "the staged upload is removed once the message is sent"
        );
    }

    #[tokio::test]
    async fn a_chat_message_naming_an_unstaged_upload_is_refused_whole() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let good_id = stage_upload(&state, "notes.txt", b"some notes").await;
        let mut inbound = user_messages(&state).await;

        let mut local = handle_on_connection(
            ClientMessage::SendMessage {
                id: "chat-2".into(),
                content: "here".into(),
                attachments: vec![good_id.clone(), "0".repeat(32)],
            },
            &state,
        )
        .await;

        let reply = next_server_message(&mut local).await;
        assert!(
            matches!(
                &reply,
                ServerMessage::MessageRefused { id, message }
                    if id == "chat-2" && message == UPLOAD_UNAVAILABLE_MESSAGE
            ),
            "expected the unavailable-file refusal, got {reply:?}"
        );

        // Nothing was published: the next message is the first event seen.
        let _local = handle_on_connection(
            ClientMessage::SendMessage {
                id: "after".into(),
                content: "next".into(),
                attachments: Vec::new(),
            },
            &state,
        )
        .await;
        let event = inbound.recv().await.unwrap().unwrap();
        assert_eq!(event.id, "after", "the refused message was not published");

        let items = residuum_workspace::inbox::list_items_or_empty(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert!(items.is_empty(), "a refused message makes no inbox item");
        assert!(
            state.uploads.resolve(&good_id).await.is_ok(),
            "the other staged upload is kept"
        );
    }

    #[tokio::test]
    async fn a_publish_failure_leaves_no_companion_item() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = make_test_gateway_state(dir.path());
        let photo_id = stage_upload(&state, "photo.txt", b"photo bytes").await;
        // A publisher not backed by a broker fails every publish, as a stopped
        // bus does.
        state.publisher = residuum_bus::Publisher::noop();

        let (keep_going, mut local) = handle_on_connection_with_outcome(
            ClientMessage::SendMessage {
                id: "chat-4".into(),
                content: "photo".into(),
                attachments: vec![photo_id.clone()],
            },
            &state,
        )
        .await;
        assert!(!keep_going, "a stopped bus ends the connection");

        let reply = next_server_message(&mut local).await;
        assert!(
            matches!(
                &reply,
                ServerMessage::MessageRefused { id, message }
                    if id == "chat-4"
                        && message == "Your message didn't reach the agent. Send it again."
            ),
            "expected the publish-failure refusal, got {reply:?}"
        );

        let items = residuum_workspace::inbox::list_items_or_empty(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert!(items.is_empty(), "the unpublished message leaves no item");
        let attachments_dir = state.agent_inbox_dir.join("attachments");
        let leftovers = if attachments_dir.exists() {
            std::fs::read_dir(&attachments_dir).unwrap().count()
        } else {
            0
        };
        assert_eq!(leftovers, 0, "no attachment directory is left behind");
        assert!(
            state.uploads.resolve(&photo_id).await.is_ok(),
            "the staged upload is kept for a resend"
        );
    }

    #[tokio::test]
    async fn a_publish_failure_refusal_reaches_the_socket_before_it_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = make_test_gateway_state(dir.path());
        // A publisher not backed by a broker fails every publish, as a stopped
        // bus does, which ends the connection.
        state.publisher = residuum_bus::Publisher::noop();
        let addr = serve_ws_handler(state).await;
        let mut socket = open_socket_past_handshake(addr, "/ws").await;

        let send = serde_json::to_string(&ClientMessage::SendMessage {
            id: "chat-5".into(),
            content: "hello".into(),
            attachments: Vec::new(),
        })
        .unwrap();
        socket.send(WireMessage::text(send)).await.unwrap();

        let refusal = serde_json::to_string(&ServerMessage::MessageRefused {
            id: "chat-5".into(),
            message: "Your message didn't reach the agent. Send it again.".into(),
        })
        .unwrap();
        loop {
            let Some(Ok(frame)) = wait::guarded("the refusal on the socket", socket.next()).await
            else {
                panic!("the socket ended before the refusal arrived");
            };
            match frame {
                WireMessage::Text(text) => {
                    if text.as_str() == refusal {
                        break;
                    }
                }
                WireMessage::Ping(_) | WireMessage::Pong(_) => {}
                other
                @ (WireMessage::Binary(_) | WireMessage::Close(_) | WireMessage::Frame(_)) => {
                    panic!("expected the refusal before the close, got {other:?}")
                }
            }
        }

        // The connection still ends after the refusal.
        loop {
            match wait::guarded("the socket to close", socket.next()).await {
                None | Some(Err(_) | Ok(WireMessage::Close(_))) => break,
                Some(Ok(
                    WireMessage::Text(_)
                    | WireMessage::Binary(_)
                    | WireMessage::Ping(_)
                    | WireMessage::Pong(_)
                    | WireMessage::Frame(_),
                )) => {}
            }
        }
    }

    #[tokio::test]
    async fn a_failed_copy_discards_the_items_already_made() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let first_id = stage_upload(&state, "first.txt", b"first").await;
        let second_id = stage_upload(&state, "second.txt", b"second").await;
        // The second file's bytes vanish after staging, so its copy fails
        // after the first has been copied into its item.
        let second_path = state.uploads.resolve(&second_id).await.unwrap().path;
        std::fs::remove_file(&second_path).unwrap();
        let mut inbound = user_messages(&state).await;

        let mut local = handle_on_connection(
            ClientMessage::SendMessage {
                id: "chat-3".into(),
                content: "both".into(),
                attachments: vec![first_id.clone(), second_id.clone()],
            },
            &state,
        )
        .await;

        let reply = next_server_message(&mut local).await;
        assert!(
            matches!(
                &reply,
                ServerMessage::MessageRefused { id, message }
                    if id == "chat-3"
                        && message == "Couldn't attach second.txt. Try sending it again."
            ),
            "expected the copy-failure refusal, got {reply:?}"
        );

        let items = residuum_workspace::inbox::list_items_or_empty(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert!(items.is_empty(), "the first file's item is discarded too");
        let attachments_dir = state.agent_inbox_dir.join("attachments");
        let leftovers = if attachments_dir.exists() {
            std::fs::read_dir(&attachments_dir).unwrap().count()
        } else {
            0
        };
        assert_eq!(leftovers, 0, "no attachment directory is left behind");

        let _local = handle_on_connection(
            ClientMessage::SendMessage {
                id: "after".into(),
                content: "next".into(),
                attachments: Vec::new(),
            },
            &state,
        )
        .await;
        let event = inbound.recv().await.unwrap().unwrap();
        assert_eq!(event.id, "after", "the refused message was not published");
        assert!(
            state.uploads.resolve(&first_id).await.is_ok(),
            "both staged uploads are kept for a retry"
        );
        assert!(state.uploads.resolve(&second_id).await.is_ok());
    }

    #[tokio::test]
    async fn a_message_sent_without_an_id_is_given_one() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        let mut inbound: residuum_bus::Subscriber<residuum_bus::MessageEvent> = state
            .bus_handle
            .subscribe(residuum_bus::topics::UserMessage)
            .await
            .unwrap();
        let (local_tx, _local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());

        let keep_going = handle_client_message(
            ClientMessage::SendMessage {
                id: String::new(),
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
            &state,
            &ConnectionChannels {
                local: &local_tx,
                verbose: &verbose,
                watch_set: &watch_tx,
                resync: &mpsc::unbounded_channel().0,
            },
        )
        .await;
        assert!(keep_going);

        let event = inbound.recv().await.unwrap().unwrap();
        assert!(
            !event.id.is_empty(),
            "an empty id would make the turn's reply look like a post outside any turn"
        );
        assert_eq!(event.content, "hello");
    }

    #[tokio::test]
    async fn inbox_add_success_notice_reads_as_plain_language() {
        let dir = tempfile::tempdir().unwrap();
        // The workspace directory's name is the agent's folder name, which a
        // mail ref must be able to carry.
        let workspace = dir.path().join("atlas");
        let state = make_test_gateway_state(&workspace);
        std::fs::create_dir_all(&state.agent_inbox_dir).unwrap();
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());

        let keep_going = handle_client_message(
            ClientMessage::InboxAdd {
                body: "remember this".to_string(),
            },
            &state,
            &ConnectionChannels {
                local: &local_tx,
                verbose: &verbose,
                watch_set: &watch_tx,
                resync: &mpsc::unbounded_channel().0,
            },
        )
        .await;
        assert!(keep_going);

        let msg = next_server_message(&mut local_rx).await;
        assert!(
            matches!(&msg, ServerMessage::Notice { message, .. } if message == "Added a note to the inbox."),
            "expected a plain-language Notice, got {msg:?}"
        );

        // The note is the user's mail to this agent, so it is a Sent thread.
        let mailbox =
            residuum_workspace::inbox::mail::Mailbox::for_workspace(&state.layout, state.tz);
        let index = residuum_workspace::inbox::mail::MailIndex::load(&mailbox).await;
        let sent: Vec<_> = index
            .threads(&residuum_workspace::inbox::mail::ThreadFilter {
                folder: Some(residuum_workspace::inbox::mail::MailFolder::Sent),
                agent: None,
            })
            .collect();
        let [thread] = sent.as_slice() else {
            panic!("one Sent thread for the note, found {}", sent.len());
        };
        assert_eq!(thread.subject(), "remember this");
        assert_eq!(thread.root().to, std::slice::from_ref(&mailbox.agent));
        let item = residuum_workspace::inbox::load_item(
            &state
                .agent_inbox_dir
                .join(format!("{}.json", thread.root().item_ref.id())),
        )
        .await
        .unwrap();
        assert_eq!(item.source, "cli", "a terminal note is stored as cli mail");
    }

    #[tokio::test]
    async fn inbox_add_failure_keeps_the_cause_out_of_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_test_gateway_state(dir.path());
        // A regular file sits where `agent_inbox_dir` belongs, so claiming an
        // item id underneath `quick_add` fails and the client should see a
        // plain-language notice with the cause moved to `details`.
        std::fs::create_dir_all(dir.path().join("inbox")).unwrap();
        std::fs::write(dir.path().join("inbox/agent"), b"not a directory").unwrap();
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let verbose = AtomicBool::new(false);
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(WatchSet::default());

        let keep_going = handle_client_message(
            ClientMessage::InboxAdd {
                body: "remember this".to_string(),
            },
            &state,
            &ConnectionChannels {
                local: &local_tx,
                verbose: &verbose,
                watch_set: &watch_tx,
                resync: &mpsc::unbounded_channel().0,
            },
        )
        .await;
        assert!(keep_going);

        let msg = next_server_message(&mut local_rx).await;
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
