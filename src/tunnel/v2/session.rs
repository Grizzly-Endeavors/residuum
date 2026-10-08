//! One v2 tunnel connection: the upgrade, the handshake, and the frame loop.
//!
//! The loop never waits on a stream consumer: inbound data is queued per
//! stream (bounded by its window) and outbound frames go through a writer
//! task fed by two queues, control frames first.

use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::frames::{AgentInfo, V2Frame, WireHosts};
use super::link::{LinkShared, RelayLink};
use super::stream::{Outbox, OutboxReceivers, PushError, StreamTable};
use super::{ConnectedInfo, IncomingStream, SessionHandler, Verdict};
use crate::remote_access::types::Hostnames;
use crate::tunnel::TunnelStatus;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How long the relay has to send `connected` after the upgrade.
const CONNECTED_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a closing session waits for its websocket close to be written.
const CLOSE_FLUSH: Duration = Duration::from_secs(2);

/// How often idle streams are looked for.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Result of the websocket upgrade on the registration endpoint.
pub(in crate::tunnel) enum ConnectOutcome {
    /// The relay accepted the upgrade.
    Connected(Box<WsStream>),
    /// The relay has no registration endpoint or wants a capability this client lacks
    /// (HTTP 404 or 426); the status is attached.
    Unsupported(u16),
    /// Any other failure (auth, network, TLS).
    Failed(WsError),
}

/// Open the tunnel websocket.
pub(in crate::tunnel) async fn connect(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
) -> ConnectOutcome {
    match tokio_tungstenite::connect_async(request).await {
        Ok((ws, _response)) => ConnectOutcome::Connected(Box::new(ws)),
        Err(WsError::Http(response)) if matches!(response.status().as_u16(), 404 | 426) => {
            ConnectOutcome::Unsupported(response.status().as_u16())
        }
        Err(e) => ConnectOutcome::Failed(e),
    }
}

/// How a session ended.
pub(in crate::tunnel) enum SessionEnd {
    /// Shutdown was requested; `Disconnected` is already published.
    Shutdown,
    /// The handler refused the relay; `Disconnected` is already published.
    Refused,
    /// The connection was lost. `accepted` tells whether the handler had
    /// accepted it first.
    Lost { reason: String, accepted: bool },
}

/// What a session needs from the tunnel client around it.
pub(in crate::tunnel) struct SessionInputs<'a> {
    pub handler: &'a Arc<dyn SessionHandler>,
    pub agents_rx: &'a mut watch::Receiver<Vec<AgentInfo>>,
    pub shutdown_rx: &'a mut watch::Receiver<bool>,
    pub status_tx: &'a watch::Sender<TunnelStatus>,
}

struct ConnectedFrame {
    user: String,
    instance: String,
    keepalive_interval_secs: u64,
    hosts: WireHosts,
    a2a_token: String,
}

async fn wait_for_connected(read: &mut SplitStream<WsStream>) -> Result<ConnectedFrame, String> {
    while let Some(message) = read.next().await {
        match message {
            Ok(Message::Text(text)) => match serde_json::from_str::<V2Frame>(&text) {
                Ok(V2Frame::Connected {
                    user,
                    instance,
                    keepalive_interval_secs,
                    hosts,
                    a2a_token,
                    ..
                }) => {
                    return Ok(ConnectedFrame {
                        user,
                        instance,
                        keepalive_interval_secs,
                        hosts,
                        a2a_token,
                    });
                }
                Ok(other) => debug!(frame = ?other, "ignoring frame before connected"),
                Err(e) => warn!(error = %e, "unparseable frame during the v2 handshake"),
            },
            Ok(Message::Close(_)) => {
                return Err("the relay closed the connection before sending connected".into());
            }
            Ok(_) => {}
            Err(e) => return Err(format!("websocket error during the handshake: {e}")),
        }
    }
    Err("the relay closed the connection before sending connected".into())
}

/// Resolves when shutdown is requested; never resolves if the sender is gone
/// without having asked.
async fn shutdown_requested(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Send queued messages to the websocket, control frames first.
async fn write_loop(mut sink: SplitSink<WsStream, Message>, mut queues: OutboxReceivers) {
    loop {
        let message = tokio::select! {
            biased;
            Some(message) = queues.control.recv() => message,
            Some(message) = queues.ordered.recv() => message,
            else => break,
        };
        let is_close = matches!(message, Message::Close(_));
        if let Err(e) = sink.send(message).await {
            debug!(error = %e, "v2 tunnel write failed");
            break;
        }
        if is_close {
            break;
        }
    }
}

fn send_agents_update(outbox: &Outbox, agents_rx: &mut watch::Receiver<Vec<AgentInfo>>) {
    let agents = agents_rx.borrow_and_update().clone();
    let count = agents.len();
    outbox.send_control(&V2Frame::AgentsUpdate { agents });
    debug!(agents = count, "sent the agent list to the relay");
}

/// The per-connection state the frame handlers share.
struct Frames<'a> {
    handler: &'a Arc<dyn SessionHandler>,
    outbox: Outbox,
    table: Arc<StreamTable>,
    link: Arc<LinkShared>,
    accepted: bool,
}

impl Frames<'_> {
    /// Handle one websocket message. Returns the reason to end the session.
    fn handle(&self, message: Message) -> Option<String> {
        match message {
            Message::Text(text) => match serde_json::from_str::<V2Frame>(&text) {
                Ok(frame) => self.handle_frame(frame),
                Err(e) => warn!(error = %e, "unparseable v2 frame"),
            },
            Message::Binary(data) => self.handle_data(&data),
            Message::Close(_) => return Some("relay sent close frame".to_string()),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
        None
    }

    fn reject(&self, stream_id: Uuid, reason: &str) {
        self.outbox.send_control(&V2Frame::StreamClose {
            stream_id,
            reason: Some(reason.to_string()),
        });
    }

    fn handle_frame(&self, frame: V2Frame) {
        match frame {
            V2Frame::Ping => self.outbox.send_control(&V2Frame::Pong),
            V2Frame::Pong => {}
            V2Frame::StreamOpen {
                stream_id,
                host,
                peer_ip,
            } => self.open_stream(stream_id, host, peer_ip),
            V2Frame::StreamClose { stream_id, .. } => {
                if let Some(stream) = self.table.get(stream_id) {
                    stream.relay_closed();
                }
            }
            V2Frame::StreamCredit { stream_id, bytes } => match self.table.get(stream_id) {
                Some(stream) => stream.add_credit(bytes),
                None => self.reject(stream_id, "unknown stream"),
            },
            V2Frame::ChallengeGranted { names } => self.link.resolve_claim(&names, true),
            V2Frame::ChallengeBusy { names } => self.link.resolve_claim(&names, false),
            V2Frame::PinGrant { purpose, grant } => {
                self.link.resolve_grant(&purpose, Ok(grant));
            }
            V2Frame::PinGrantError { purpose, reason } => {
                self.link.resolve_grant(&purpose, Err(reason));
            }
            V2Frame::InstancesUpdate { instances } => {
                debug!(count = instances.len(), "relay sent the instance list");
                self.handler.on_instances(instances);
            }
            other @ (V2Frame::Connected { .. }
            | V2Frame::AgentsUpdate { .. }
            | V2Frame::ChallengeClaim { .. }
            | V2Frame::ChallengeRelease { .. }
            | V2Frame::ActivateInstance { .. }
            | V2Frame::PinGrantRequest { .. }) => {
                warn!(frame = ?other, "unexpected v2 frame from the relay");
            }
        }
    }

    fn open_stream(&self, stream_id: Uuid, host: String, peer_ip: String) {
        if !self.accepted {
            self.reject(stream_id, "instance not ready");
            return;
        }
        match self.table.open(stream_id) {
            Ok(io) => self.handler.on_stream(IncomingStream { host, peer_ip, io }),
            Err(super::stream::OpenError::TooManyStreams) => {
                warn!(%stream_id, "too many concurrent tunnel streams; refusing a new one");
                self.reject(stream_id, "too many streams");
            }
            Err(super::stream::OpenError::Duplicate) => {
                warn!(%stream_id, "relay reused the id of a live stream; ignored");
            }
            Err(super::stream::OpenError::Closed) => self.reject(stream_id, "tunnel closing"),
        }
    }

    fn handle_data(&self, data: &[u8]) {
        let Some((id_bytes, payload)) = data.split_first_chunk::<16>() else {
            warn!("v2 data message shorter than a stream id");
            return;
        };
        let stream_id = Uuid::from_bytes(*id_bytes);
        let Some(stream) = self.table.get(stream_id) else {
            self.reject(stream_id, "unknown stream");
            return;
        };
        let reason = match stream.push_data(payload) {
            Ok(()) => return,
            Err(PushError::WindowExceeded) => "send window exceeded",
            Err(PushError::TooLarge) => "data message over 64 KiB",
        };
        warn!(%stream_id, reason, "relay broke the stream protocol; closing the stream");
        self.table.remove(stream_id);
        stream.reset(reason, true);
    }
}

/// Wait for the relay's `connected` frame, or end the session trying.
async fn handshake(
    read: &mut SplitStream<WsStream>,
    shutdown_rx: &mut watch::Receiver<bool>,
    status_tx: &watch::Sender<TunnelStatus>,
) -> Result<ConnectedFrame, SessionEnd> {
    let waited = tokio::select! {
        result = tokio::time::timeout(CONNECTED_TIMEOUT, wait_for_connected(read)) => result,
        () = shutdown_requested(shutdown_rx) => {
            publish_disconnected(status_tx);
            return Err(SessionEnd::Shutdown);
        }
    };
    match waited {
        Ok(Ok(hello)) => Ok(hello),
        Ok(Err(reason)) => {
            warn!(reason = %reason, "v2 handshake failed");
            Err(SessionEnd::Lost {
                reason,
                accepted: false,
            })
        }
        Err(_) => {
            warn!("timed out waiting for the v2 connected frame from the relay");
            Err(SessionEnd::Lost {
                reason: "timed out waiting for connected".to_string(),
                accepted: false,
            })
        }
    }
}

/// Publish an accepted relay's identity and origins.
fn publish_accepted(
    status_tx: &watch::Sender<TunnelStatus>,
    info: &ConnectedInfo,
    verdict: AcceptedRelay,
) {
    info!(user = %verdict.user, instance = %verdict.instance, keepalive_interval_secs = info.keepalive_interval_secs, "tunnel connected");
    status_tx
        .send(TunnelStatus::Connected {
            user_id: verdict.user,
            origin: verdict.ui_origin,
            workbench_origin: verdict.workbench_origin,
            instance: Some(verdict.instance),
            instance_origin: verdict.instance_origin,
        })
        .unwrap_or_else(|_| debug!("status receiver dropped"));
}

struct AcceptedRelay {
    user: String,
    instance: String,
    ui_origin: Option<String>,
    workbench_origin: Option<String>,
    instance_origin: Option<String>,
}

/// Tear a session down: fail waiters and streams, say goodbye when the end
/// was ours, and stop the writer.
async fn wind_down(
    end: &SessionEnd,
    outbox: &Outbox,
    link: &LinkShared,
    table: &StreamTable,
    status_tx: &watch::Sender<TunnelStatus>,
    mut writer: tokio::task::JoinHandle<()>,
) {
    link.close();
    table.close_all();
    if matches!(end, SessionEnd::Shutdown | SessionEnd::Refused) {
        outbox.send_control_message(Message::Close(None));
        publish_disconnected(status_tx);
        if tokio::time::timeout(CLOSE_FLUSH, &mut writer)
            .await
            .is_err()
        {
            warn!("timed out writing the websocket close; dropping the connection");
        }
    }
    writer.abort();
}

impl SessionEnd {
    fn lost(reason: impl Into<String>, accepted: bool) -> Self {
        Self::Lost {
            reason: reason.into(),
            accepted,
        }
    }
}

/// What the relay announced, as the handler is shown it.
fn connected_info(hello: ConnectedFrame) -> ConnectedInfo {
    ConnectedInfo {
        user: hello.user,
        instance: hello.instance,
        hosts: Hostnames {
            ui: hello.hosts.ui,
            workbench: hello.hosts.workbench,
            instance: hello.hosts.instance,
        },
        keepalive_interval_secs: hello.keepalive_interval_secs,
        a2a_token: hello.a2a_token,
    }
}

/// Run one v2 session on an upgraded websocket until it ends.
pub(in crate::tunnel) async fn run_session(ws: WsStream, inputs: SessionInputs<'_>) -> SessionEnd {
    let SessionInputs {
        handler,
        agents_rx,
        shutdown_rx,
        status_tx,
    } = inputs;
    let (sink, mut read) = ws.split();

    let hello = match handshake(&mut read, shutdown_rx, status_tx).await {
        Ok(hello) => hello,
        Err(end) => return end,
    };

    let (outbox, queues) = Outbox::new();
    let mut writer = crate::util::spawn_in_span(write_loop(sink, queues));
    let table = StreamTable::new(outbox.clone());
    let link_shared = LinkShared::new(outbox.clone());
    let link = RelayLink::new(Arc::clone(&link_shared));
    let info = connected_info(hello);
    let keepalive_timeout = Duration::from_secs(info.keepalive_interval_secs.max(1) * 3);

    let mut frames = Frames {
        handler,
        outbox: outbox.clone(),
        table: Arc::clone(&table),
        link: Arc::clone(&link_shared),
        accepted: false,
    };
    let end = {
        let mut verdict = pin!(handler.on_connected(&info, link));
        let mut verdict_done = false;
        let mut agents_open = true;
        let mut last_frame = Instant::now();
        let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                message = read.next() => match message {
                    Some(Ok(message)) => {
                        last_frame = Instant::now();
                        if let Some(reason) = frames.handle(message) {
                            break SessionEnd::Lost { reason, accepted: frames.accepted };
                        }
                    }
                    Some(Err(e)) => {
                        break SessionEnd::lost(format!("websocket error: {e}"), frames.accepted);
                    }
                    None => {
                        break SessionEnd::lost("websocket stream ended", frames.accepted);
                    }
                },
                decision = &mut verdict, if !verdict_done => {
                    verdict_done = true;
                    match decision {
                        Verdict::Accept { user, instance, ui_origin, workbench_origin, instance_origin } => {
                            frames.accepted = true;
                            publish_accepted(
                                status_tx,
                                &info,
                                AcceptedRelay { user, instance, ui_origin, workbench_origin, instance_origin },
                            );
                            send_agents_update(&outbox, agents_rx);
                        }
                        Verdict::Refuse(reason) => {
                            error!(reason = %reason, relay_user = %info.user, relay_instance = %info.instance, "refusing the relay's tunnel");
                            break SessionEnd::Refused;
                        }
                    }
                }
                changed = agents_rx.changed(), if frames.accepted && agents_open => {
                    if changed.is_ok() {
                        send_agents_update(&outbox, agents_rx);
                    } else {
                        agents_open = false;
                        debug!("agent list sender dropped; no further agent updates will be sent");
                    }
                }
                () = shutdown_requested(shutdown_rx) => {
                    info!("tunnel shutting down");
                    break SessionEnd::Shutdown;
                }
                () = tokio::time::sleep_until(last_frame + keepalive_timeout) => {
                    break SessionEnd::Lost {
                        reason: "keepalive timeout".to_string(),
                        accepted: frames.accepted,
                    };
                }
                _ = sweep.tick() => table.sweep_idle(),
                _ = &mut writer => {
                    break SessionEnd::Lost {
                        reason: "websocket write failed".to_string(),
                        accepted: frames.accepted,
                    };
                }
            }
        }
    };

    wind_down(&end, &outbox, &link_shared, &table, status_tx, writer).await;
    handler.on_disconnected();
    end
}

fn publish_disconnected(status_tx: &watch::Sender<TunnelStatus>) {
    status_tx
        .send(TunnelStatus::Disconnected)
        .unwrap_or_else(|_| debug!("status receiver dropped"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_requested_waits_for_the_flag() {
        let (tx, mut rx) = watch::channel(false);
        let waiter = tokio::spawn(async move { shutdown_requested(&mut rx).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }
}
