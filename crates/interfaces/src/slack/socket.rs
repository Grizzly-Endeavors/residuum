//! Slack Socket Mode client — connect, read envelopes, ack-first, reconnect.
//!
//! Owns one WebSocket per Slack connection: every cycle starts with a fresh
//! `apps.connections.open` (the wss URL rotates between connections), reads
//! `Message::Text` frames as [`Envelope`]s, and acknowledges *before*
//! dispatching by writing `{"envelope_id": "<id>"}` back on the same
//! websocket (Slack docs, <https://docs.slack.dev/apis/events-api/using-socket-mode>
//! §5 "Acknowledge events"). Slack retries un-acked events — under a fresh
//! `envelope_id` — and upstream dedup on the message identity absorbs the
//! overlap (design doc, Lifecycle §4). A `disconnect` envelope
//! reconnects immediately; socket errors and closes reconnect through capped
//! exponential backoff that resets once `hello` says the new connection is
//! live. The loop shapes mirror the Telegram adapter
//! (`telegram/handler.rs`'s polling cycle and `boot_retry.rs`'s transition
//! logging).
//!
//! Shutdown: the interface constructor (`mod.rs`) owns the spawned task and
//! aborts it on teardown, the same guard pattern as the Telegram outbound
//! subscriber.

use std::time::Duration;

use anyhow::Context as _;
use futures_util::{Sink, SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::api::SlackApi;
use super::events::{Envelope, EnvelopeKind};

/// Reconnect backoff after a failed connect or read cycle; it doubles from
/// here on each consecutive failure.
const BACKOFF_START: Duration = Duration::from_secs(1);
/// Ceiling the reconnect backoff doubles up to, but never past.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Liveness (Task 4 review I-1): tokio-tungstenite 0.26's connect helpers
/// build their own `TcpStream` with no hook to set TCP keepalive on it (the
/// Telegram adapter's `tcp_keepalive(60s)` route is unreachable for us), so
/// dead-connection detection is client-side at the websocket protocol level
/// instead: a ping every 30 s — conservative next to the official Slack
/// SDKs' ~5 s — plus the read-silence timeout below. A silently-dropped
/// connection (NAT idle timeout, half-open TCP) receives no frame and no
/// pong; the timeout turns that into an error, which flows into the
/// existing reconnect path with backoff.
const KEEPALIVE_PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long the connection may go without any inbound frame — data, ping,
/// or pong — before it is declared dead. Three ping intervals (spelled out:
/// `Duration` multiplication is not const): a healthy connection answers
/// each of our pings with a pong and Slack sends its own pings besides, so
/// real silence for this long means the connection is gone. Bounds a silent
/// drop's detection at ~90 s, well inside "minutes".
const READ_SILENCE_TIMEOUT: Duration = Duration::from_secs(90);

/// The connected socket [`tokio_tungstenite::connect_async`] yields.
type SlackSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Run the Socket Mode client until the task is aborted.
///
/// Every `events_api` envelope is dispatched to `on_envelope` **after** its
/// ack frame has been written to the socket (ack-first). `hello` and
/// `disconnect` are connection lifecycle, not conversation: handled here,
/// never acked (they carry no `envelope_id`), never dispatched. Unknown
/// envelope types are logged, acked, and skipped (design D4).
///
/// # Errors
/// Never in practice: every connect or read failure reconnects forever with
/// capped backoff instead of surfacing. The `Result` is kept so the
/// interface's task plumbing (`mod.rs`) stays unchanged.
pub(crate) async fn run_socket<A, F>(api: A, mut on_envelope: F) -> anyhow::Result<()>
where
    A: SlackApi + 'static,
    F: FnMut(Envelope) + Send + 'static,
{
    let mut backoff = BACKOFF_START;
    let mut consecutive_failures: u32 = 0;
    let mut ever_connected = false;

    loop {
        // Every cycle starts with a fresh apps.connections.open — Slack
        // rotates the wss host between connections.
        let socket = match connect_once(&api).await {
            Ok(socket) => socket,
            Err(error) => {
                back_off_and_retry(&mut consecutive_failures, &mut backoff, error).await;
                continue;
            }
        };

        match read_cycle(
            socket,
            &mut on_envelope,
            &mut consecutive_failures,
            &mut backoff,
            &mut ever_connected,
        )
        .await
        {
            Cycle::GracefulDisconnect => {
                tracing::info!("slack socket: disconnect envelope — reconnecting immediately");
            }
            Cycle::Lost(error) => {
                back_off_and_retry(&mut consecutive_failures, &mut backoff, error).await;
            }
        }
    }
}

/// Open a fresh Socket Mode connection: `apps.connections.open` for the
/// URL, then the WebSocket handshake to it.
///
/// # Errors
/// Propagates either step's failure; the reconnect loop owns retrying.
async fn connect_once(api: &impl SlackApi) -> anyhow::Result<SlackSocket> {
    let url = api
        .open_connection()
        .await
        .context("apps.connections.open failed")?;
    let (socket, _response) = tokio_tungstenite::connect_async(url)
        .await
        .context("failed to open the socket mode websocket")?;
    Ok(socket)
}

/// Why a read cycle ended.
enum Cycle {
    /// Slack sent a `disconnect` envelope: reconnect now, without sleeping
    /// or growing the backoff.
    GracefulDisconnect,
    /// The connection died — read error, close frame, or EOF. Reconnect
    /// through backoff.
    Lost(anyhow::Error),
}

/// Read envelopes off one live connection until it ends, acking each
/// `events_api` and unknown envelope on this same socket before it is
/// dispatched or skipped.
///
/// Liveness runs alongside the reads (see [`KEEPALIVE_PING_INTERVAL`]): the
/// socket is split so keepalive pings and pongs can be written while a read
/// is pending, and the read-silence timeout — refreshed by every inbound
/// frame — returns [`Cycle::Lost`] for a connection that has gone quiet,
/// feeding the caller's reconnect path exactly like a read error.
async fn read_cycle<F>(
    socket: SlackSocket,
    on_envelope: &mut F,
    consecutive_failures: &mut u32,
    backoff: &mut Duration,
    ever_connected: &mut bool,
) -> Cycle
where
    F: FnMut(Envelope),
{
    let (mut sink, mut stream) = socket.split();
    let mut next_ping = tokio::time::Instant::now() + KEEPALIVE_PING_INTERVAL;
    let mut silence_deadline = tokio::time::Instant::now() + READ_SILENCE_TIMEOUT;
    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Cycle::Lost(anyhow::anyhow!("slack socket stream ended"));
                };
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        return Cycle::Lost(anyhow::Error::new(error).context("socket read failed"));
                    }
                };
                silence_deadline = tokio::time::Instant::now() + READ_SILENCE_TIMEOUT;
                match frame {
                    Message::Text(text) => {
                        let envelope: Envelope = match serde_json::from_str(text.as_str()) {
                            Ok(envelope) => envelope,
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "slack socket: unparseable frame — skipping it, the connection stays up"
                                );
                                continue;
                            }
                        };
                        match &envelope.kind {
                            EnvelopeKind::EventsApi | EnvelopeKind::Unknown(_) => {
                                ack_then_dispatch(&mut sink, envelope, &mut *on_envelope).await;
                            }
                            EnvelopeKind::Hello => {
                                if *consecutive_failures > 0 {
                                    tracing::info!(
                                        attempts = *consecutive_failures,
                                        "slack socket reconnected"
                                    );
                                } else if !*ever_connected {
                                    tracing::info!("slack socket connected");
                                }
                                *ever_connected = true;
                                *consecutive_failures = 0;
                                *backoff = BACKOFF_START;
                            }
                            EnvelopeKind::Disconnect => return Cycle::GracefulDisconnect,
                        }
                    }
                    // Slack pings the client; answer or it drops the connection.
                    Message::Ping(payload) => {
                        if let Err(error) = sink.send(Message::Pong(payload)).await {
                            return Cycle::Lost(anyhow::Error::new(error).context("socket pong failed"));
                        }
                    }
                    Message::Close(_) => {
                        return Cycle::Lost(anyhow::anyhow!("slack socket closed by the server"));
                    }
                    Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                }
            }
            () = tokio::time::sleep_until(next_ping) => {
                // A protocol-level keepalive ping (RFC 6455 allows any
                // payload; an empty one is the cheapest heartbeat). A send
                // failure means the write side is dead — same reconnect
                // path as a read failure.
                if let Err(error) = sink
                    .send(Message::Ping(tokio_tungstenite::tungstenite::Bytes::new()))
                    .await
                {
                    return Cycle::Lost(anyhow::Error::new(error).context("socket keepalive ping failed"));
                }
                next_ping = tokio::time::Instant::now() + KEEPALIVE_PING_INTERVAL;
            }
            () = tokio::time::sleep_until(silence_deadline) => {
                return Cycle::Lost(anyhow::anyhow!(
                    "slack socket read silent for {READ_SILENCE_TIMEOUT:?} — treating the connection as dead and reconnecting"
                ));
            }
        }
    }
}

/// Ack one envelope on the socket, then hand it on: `events_api` envelopes
/// go to `on_envelope`, unknown types are logged and skipped (design D4).
///
/// Ack-first (design, Lifecycle §4; Slack docs §5): the ack frame is written
/// before dispatch, so a slow handler can never delay it. Generic over the
/// sink so the ordering is unit-testable without a network.
async fn ack_then_dispatch<S, F>(sink: &mut S, envelope: Envelope, on_envelope: &mut F)
where
    S: Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
    F: FnMut(Envelope),
{
    ack(sink, &envelope).await;
    match &envelope.kind {
        EnvelopeKind::EventsApi => on_envelope(envelope),
        EnvelopeKind::Unknown(kind) => {
            tracing::debug!(
                envelope_type = %kind,
                envelope_id = %envelope.id,
                "slack socket: unknown envelope type — acked, then skipped"
            );
        }
        EnvelopeKind::Hello | EnvelopeKind::Disconnect => {}
    }
}

/// Send the Socket Mode ack — the text frame `{"envelope_id": "<id>"}` —
/// on the socket the envelope arrived on.
///
/// Envelopes without an `envelope_id` cannot be acked and are not. For
/// `hello` and `disconnect` that is the documented shape (debug); for any
/// other type it contradicts the docs, so it warns — Slack will retry the
/// un-ackable event — but is still tolerated, never an error. A failed send warns and moves on:
/// if the write side is dead the next read fails and reconnects, and Slack's
/// retry is absorbed by the handler's message-identity dedup.
async fn ack<S>(sink: &mut S, envelope: &Envelope)
where
    S: Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    if envelope.id.is_empty() {
        match envelope.kind {
            EnvelopeKind::Hello | EnvelopeKind::Disconnect => {
                tracing::debug!(
                    envelope_type = ?envelope.kind,
                    "slack socket: lifecycle envelope carries no envelope_id — nothing to ack"
                );
            }
            EnvelopeKind::EventsApi | EnvelopeKind::Unknown(_) => {
                tracing::warn!(
                    envelope_type = ?envelope.kind,
                    "slack socket: event envelope carries no envelope_id — cannot ack it, Slack may retry; processing it anyway"
                );
            }
        }
        return;
    }
    let frame = serde_json::json!({ "envelope_id": envelope.id }).to_string();
    if let Err(error) = sink.send(Message::text(frame)).await {
        tracing::warn!(
            envelope_id = %envelope.id,
            error = %error,
            "slack socket: ack send failed — Slack will retry; message dedup absorbs it"
        );
    }
}

/// Log a lost connection, sleep out the current backoff, then grow it.
///
/// First failure warns, repeats stay at debug, so healthy cycling never
/// spams — the same transition discipline as `boot_retry::retry_connect`
/// and the Telegram polling loop.
async fn back_off_and_retry(
    consecutive_failures: &mut u32,
    backoff: &mut Duration,
    error: anyhow::Error,
) {
    *consecutive_failures += 1;
    if *consecutive_failures == 1 {
        tracing::warn!(
            error = %error,
            "slack socket connection lost — reconnecting with backoff"
        );
    } else {
        tracing::debug!(
            attempt = *consecutive_failures,
            error = %error,
            "slack socket still failing to reconnect"
        );
    }
    tokio::time::sleep(*backoff).await;
    *backoff = grow_backoff(*backoff);
}

/// Double `current`, capped at [`BACKOFF_MAX`].
fn grow_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use crate::slack::api::SlackApiError;
    use residuum_core::testing::wait;

    /// What the driver observes: connections accepted, ack frames received
    /// over the websocket, envelopes dispatched to the callback.
    #[derive(Debug, Clone)]
    enum Observed {
        /// A ws connection was accepted; the driver steers it through `cmds`.
        Connected {
            cmds: mpsc::UnboundedSender<ServerCmd>,
        },
        /// The client sent a text frame on the websocket — the Socket Mode
        /// ack, `{"envelope_id": "<id>"}` (docs §5 "Acknowledge events").
        AckFrame { body: String },
        /// `run_socket` dispatched an envelope to the callback.
        Dispatch { id: String },
        /// The client answered the server's ping — proof its read loop is
        /// live and every frame sent before the ping was processed.
        Pong,
        /// The client sent one of its keepalive pings (liveness, Task 4
        /// review I-1) — proof the ping cadence runs while the connection
        /// is otherwise quiet.
        ClientPing,
    }

    /// Driver → live fake-Slack connection.
    #[derive(Debug)]
    enum ServerCmd {
        /// Send one text frame (an envelope JSON).
        Text(String),
        /// Send a websocket ping (the client must answer with a pong).
        Ping,
        /// Drop the socket without a close frame (abrupt failure).
        Close,
    }

    #[derive(Clone)]
    struct FakeSlackShared {
        obs: mpsc::UnboundedSender<Observed>,
    }

    /// The fake Slack: accepts ws connections the driver scripts, and
    /// records every text frame the client sends back on the socket (acks).
    async fn slack_socket_route(
        upgrade: WebSocketUpgrade,
        State(shared): State<FakeSlackShared>,
    ) -> impl IntoResponse {
        upgrade.on_upgrade(move |mut socket: WebSocket| async move {
            let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
            shared.obs
                .send(Observed::Connected { cmds })
                .expect("the driver is still observing");
            loop {
                tokio::select! {
                    command = cmd_rx.recv() => {
                        match command {
                            Some(ServerCmd::Text(text)) => {
                                if socket.send(WsMessage::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            Some(ServerCmd::Ping) => {
                                if socket.send(WsMessage::Ping("fake-slack".into())).await.is_err() {
                                    break;
                                }
                            }
                            Some(ServerCmd::Close) | None => break,
                        }
                    }
                    frame = socket.recv() => {
                        match frame {
                            None | Some(Err(_) | Ok(WsMessage::Close(_))) => break,
                            Some(Ok(WsMessage::Pong(_))) => {
                                shared.obs
                                    .send(Observed::Pong)
                                    .expect("the driver is still observing");
                            }
                            Some(Ok(WsMessage::Ping(_))) => {
                                shared.obs
                                    .send(Observed::ClientPing)
                                    .expect("the driver is still observing");
                            }
                            Some(Ok(WsMessage::Text(text))) => {
                                shared.obs
                                    .send(Observed::AckFrame { body: text.to_string() })
                                    .expect("the driver is still observing");
                            }
                            Some(Ok(WsMessage::Binary(_))) => {}
                        }
                    }
                }
            }
        })
    }

    /// A running fake Slack on a loopback port.
    struct FakeSlack {
        /// What the fake `SlackApi::open_connection` returns.
        ws_url: String,
        obs_tx: mpsc::UnboundedSender<Observed>,
        obs_rx: mpsc::UnboundedReceiver<Observed>,
    }

    async fn spawn_fake_slack() -> FakeSlack {
        let (obs_tx, obs_rx) = mpsc::unbounded_channel();
        let router = Router::new()
            .route("/ws", get(slack_socket_route))
            .with_state(FakeSlackShared {
                obs: obs_tx.clone(),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake slack");
        let addr = listener.local_addr().expect("fake slack address");
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
        FakeSlack {
            ws_url: format!("ws://{addr}/ws"),
            obs_tx,
            obs_rx,
        }
    }

    impl FakeSlack {
        /// Drives `run_socket` against the fake, recording every dispatch.
        fn drive(self, api: ScriptedApi) -> Harness {
            let dispatch_tx = self.obs_tx;
            Harness {
                client: tokio::spawn(run_socket(api, move |envelope| {
                    dispatch_tx
                        .send(Observed::Dispatch { id: envelope.id })
                        .expect("the driver is still observing");
                })),
                obs_rx: self.obs_rx,
                log: Vec::new(),
            }
        }
    }

    /// The client under test plus everything the driver observes from it.
    struct Harness {
        client: tokio::task::JoinHandle<anyhow::Result<()>>,
        obs_rx: mpsc::UnboundedReceiver<Observed>,
        /// Everything observed so far, in order — for end-of-test assertions.
        log: Vec<Observed>,
    }

    impl Harness {
        /// The next observation, failing fast (never hanging) if the client
        /// misbehaves: the hang guard bounds the wait, and a `run_socket`
        /// that exits early fails the test immediately.
        async fn next(&mut self, what: &str) -> Observed {
            let observed = tokio::select! {
                biased;
                observed = wait::guarded(what, self.obs_rx.recv()) => {
                    observed.unwrap_or_else(|| panic!("the fake slack server went away while waiting for {what}"))
                }
                result = &mut self.client => {
                    panic!("run_socket exited while waiting for {what}: {result:?}")
                }
            };
            self.log.push(observed.clone());
            observed
        }

        async fn connected(&mut self, what: &str) -> mpsc::UnboundedSender<ServerCmd> {
            let observed = self.next(what).await;
            match observed {
                Observed::Connected { cmds } => cmds,
                Observed::AckFrame { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong
                | Observed::ClientPing => panic!(
                    "expected a socket connection while waiting for {what}, saw {observed:?}"
                ),
            }
        }

        /// The next ack frame, parsed: the `envelope_id` it acks, plus the
        /// whole frame as JSON for shape assertions.
        async fn ack_frame(&mut self, what: &str) -> (String, serde_json::Value) {
            let observed = self.next(what).await;
            match observed {
                Observed::AckFrame { body } => {
                    let frame: serde_json::Value =
                        serde_json::from_str(&body).expect("the ack frame is JSON");
                    let id = frame
                        .get("envelope_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| panic!("the ack frame names an envelope_id: {body}"))
                        .to_string();
                    (id, frame)
                }
                Observed::Connected { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong
                | Observed::ClientPing => {
                    panic!("expected an ack frame while waiting for {what}, saw {observed:?}")
                }
            }
        }

        async fn pong(&mut self, what: &str) {
            let observed = self.next(what).await;
            match observed {
                Observed::Pong => {}
                Observed::Connected { .. }
                | Observed::AckFrame { .. }
                | Observed::Dispatch { .. }
                | Observed::ClientPing => {
                    panic!("expected the client's pong while waiting for {what}, saw {observed:?}");
                }
            }
        }

        async fn client_ping(&mut self, what: &str) {
            let observed = self.next(what).await;
            match observed {
                Observed::ClientPing => {}
                Observed::Connected { .. }
                | Observed::AckFrame { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong => {
                    panic!(
                        "expected a client keepalive ping while waiting for {what}, saw {observed:?}"
                    );
                }
            }
        }

        /// The ack frame and the dispatch for one `events_api` envelope, in
        /// whichever order they reach the driver: the ack crosses the wire
        /// to the fake server while the dispatch is a local callback, so
        /// their *arrival* order here is a race. The client-side order (ack
        /// written before dispatch) is proven deterministically by
        /// `acks_on_the_socket_before_dispatching`.
        async fn acked_and_dispatched(&mut self, id: &str) {
            let mut acked = false;
            let mut dispatched = false;
            while !(acked && dispatched) {
                let observed = self
                    .next(&format!("the ack frame and dispatch of {id}"))
                    .await;
                match observed {
                    Observed::AckFrame { body } => {
                        assert!(!acked, "{id} was acked twice: {body}");
                        let frame: serde_json::Value =
                            serde_json::from_str(&body).expect("the ack frame is JSON");
                        assert_eq!(
                            frame,
                            expected_ack(id),
                            "the ack is exactly {{envelope_id}}"
                        );
                        acked = true;
                    }
                    Observed::Dispatch { id: seen } => {
                        assert!(!dispatched, "{id} was dispatched twice");
                        assert_eq!(seen, id);
                        dispatched = true;
                    }
                    Observed::Connected { .. } | Observed::Pong | Observed::ClientPing => {
                        panic!("expected {id}'s ack frame or dispatch, saw {observed:?}");
                    }
                }
            }
        }
    }

    /// `SlackApi` fake: every `open_connection` is recorded (tokio clock, so
    /// paused-clock tests measure backoff gaps), and a script of results
    /// plays out before it falls back to the live URL forever.
    #[derive(Clone, Default)]
    struct ScriptedApi {
        ws_url: String,
        opens: Arc<std::sync::Mutex<Vec<tokio::time::Instant>>>,
        script: Arc<std::sync::Mutex<std::collections::VecDeque<anyhow::Result<String>>>>,
    }

    impl ScriptedApi {
        fn live(ws_url: String) -> Self {
            Self {
                ws_url,
                ..Self::default()
            }
        }

        fn scripted(ws_url: String, failures: Vec<anyhow::Error>) -> Self {
            Self {
                ws_url,
                script: Arc::new(std::sync::Mutex::new(
                    failures.into_iter().map(Err).collect(),
                )),
                ..Self::default()
            }
        }

        fn opens(&self) -> Vec<tokio::time::Instant> {
            self.opens.lock().expect("opens lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl SlackApi for ScriptedApi {
        async fn post_message(
            &self,
            _channel: &str,
            _thread_ts: Option<&str>,
            _text: &str,
        ) -> Result<(), SlackApiError> {
            panic!("run_socket never posts messages; acks ride the websocket");
        }

        async fn add_reaction(
            &self,
            _channel: &str,
            _ts: &str,
            _name: &str,
        ) -> Result<(), SlackApiError> {
            panic!("run_socket never reacts; acks ride the websocket");
        }

        async fn remove_reaction(
            &self,
            _channel: &str,
            _ts: &str,
            _name: &str,
        ) -> Result<(), SlackApiError> {
            panic!("run_socket never reacts; acks ride the websocket");
        }

        async fn open_connection(&self) -> anyhow::Result<String> {
            self.opens
                .lock()
                .expect("opens lock")
                .push(tokio::time::Instant::now());
            match self.script.lock().expect("script lock").pop_front() {
                Some(result) => result,
                None => Ok(self.ws_url.clone()),
            }
        }
    }

    /// The hello fixture text — no `envelope_id`.
    fn hello_text() -> String {
        include_str!("fixtures/envelope-hello.json").to_string()
    }

    /// The corrected disconnect fixture — `{"type":"disconnect",…}` with no
    /// `envelope_id` and no payload.
    fn disconnect_text() -> String {
        include_str!("fixtures/envelope-disconnect.json").to_string()
    }

    /// The `events_api` fixture (documented shape: no top-level
    /// `response_url`) with its identity swapped.
    fn events_envelope(id: &str) -> String {
        rewire(
            include_str!("fixtures/envelope-events-api-message.json"),
            id,
        )
    }

    /// The unknown-type (`slash_commands`) fixture, likewise rewired.
    fn unknown_envelope(id: &str) -> String {
        rewire(include_str!("fixtures/envelope-unknown.json"), id)
    }

    fn rewire(fixture: &str, id: &str) -> String {
        let mut envelope: serde_json::Value =
            serde_json::from_str(fixture).expect("fixture parses");
        let object = envelope
            .as_object_mut()
            .expect("every envelope fixture is a JSON object");
        object.insert(
            "envelope_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
        serde_json::to_string(&envelope).expect("rewired fixture serializes")
    }

    /// The ack frame the docs (§5) prescribe: exactly `{"envelope_id": id}`.
    fn expected_ack(id: &str) -> serde_json::Value {
        serde_json::json!({ "envelope_id": id })
    }

    #[tokio::test]
    async fn acks_each_events_envelope_over_the_websocket_then_reconnects_on_disconnect() {
        let fake = spawn_fake_slack().await;
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        // Connection one: hello (never acked: no envelope_id), then a DM
        // event the client must ack with a websocket frame.
        let conn1 = harness.connected("the first socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn1
            .send(ServerCmd::Text(events_envelope("env-evt-1")))
            .expect("the scripted connection is still open");
        harness.acked_and_dispatched("env-evt-1").await;

        // Graceful disconnect → immediate reconnect → acks ride the NEW socket.
        conn1
            .send(ServerCmd::Text(disconnect_text()))
            .expect("the scripted connection is still open");
        let conn2 = harness.connected("the reconnect after disconnect").await;
        conn2
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn2
            .send(ServerCmd::Text(events_envelope("env-evt-2")))
            .expect("the scripted connection is still open");
        harness.acked_and_dispatched("env-evt-2").await;

        // A ping round-trip after everything: frames on one connection are
        // ordered, so the pong proves no stray ack (hello/disconnect) is
        // still in flight behind the ones counted.
        conn2
            .send(ServerCmd::Ping)
            .expect("the scripted connection is still open");
        harness.pong("the pong that flushes the connection").await;

        harness.client.abort();
        let log = &harness.log;
        let connections = log
            .iter()
            .filter(|observed| matches!(observed, Observed::Connected { .. }))
            .count();
        assert_eq!(connections, 2, "both connections were seen: {log:?}");
        let acks = log
            .iter()
            .filter(|observed| matches!(observed, Observed::AckFrame { .. }))
            .count();
        assert_eq!(
            acks, 2,
            "exactly one ack per events_api envelope; hello and disconnect are never acked: {log:?}"
        );
        assert!(
            !log.iter().any(|observed| matches!(
                observed,
                Observed::Dispatch { id } if id.is_empty()
            )),
            "hello/disconnect carry no envelope_id and must never be dispatched: {log:?}"
        );
    }

    #[tokio::test]
    async fn unknown_envelopes_are_acked_over_the_socket_and_never_dispatched() {
        let fake = spawn_fake_slack().await;
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        let conn1 = harness.connected("the socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");

        // An unknown type (slash_commands) still carries an envelope_id:
        // acked (or Slack retries it forever, D4), never dispatched.
        conn1
            .send(ServerCmd::Text(unknown_envelope("env-unknown-1")))
            .expect("the scripted connection is still open");
        let (ack_id, frame) = harness.ack_frame("the ack for the unknown envelope").await;
        assert_eq!(ack_id, "env-unknown-1");
        assert_eq!(frame, expected_ack("env-unknown-1"));

        // A following events_api envelope proves the unknown one was
        // handled in order without being dispatched.
        conn1
            .send(ServerCmd::Text(events_envelope("env-evt-after")))
            .expect("the scripted connection is still open");
        harness.acked_and_dispatched("env-evt-after").await;

        harness.client.abort();
        let log = &harness.log;
        assert!(
            !log.iter().any(|observed| matches!(
                observed,
                Observed::Dispatch { id: seen } if seen == "env-unknown-1"
            )),
            "unknown envelope types are acked and skipped, never dispatched: {log:?}"
        );
    }

    #[tokio::test]
    async fn a_malformed_text_frame_is_skipped_and_the_connection_stays_up() {
        let fake = spawn_fake_slack().await;
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        let conn1 = harness.connected("the socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        // Mid-stream garbage: not JSON at all. The client warns, skips the
        // frame, and keeps reading the same connection (Task 4 review I-2).
        conn1
            .send(ServerCmd::Text("not json".to_string()))
            .expect("the scripted connection is still open");
        conn1
            .send(ServerCmd::Text(events_envelope("env-after-garbage")))
            .expect("the scripted connection is still open");
        harness.acked_and_dispatched("env-after-garbage").await;

        harness.client.abort();
        let log = &harness.log;
        let connections = log
            .iter()
            .filter(|observed| matches!(observed, Observed::Connected { .. }))
            .count();
        assert_eq!(
            connections, 1,
            "a malformed frame must not drop the connection: {log:?}"
        );
        assert!(
            !log.iter().any(|observed| matches!(
                observed,
                Observed::Dispatch { id } if id.is_empty()
            )),
            "the garbage frame dispatched nothing: {log:?}"
        );
    }

    #[tokio::test]
    async fn an_abrupt_close_reconnects_like_a_disconnect_does() {
        let fake = spawn_fake_slack().await;
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        let conn1 = harness.connected("the first socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        // The pong proves the client's read loop is live on this connection.
        conn1
            .send(ServerCmd::Ping)
            .expect("the scripted connection is still open");
        harness.pong("the pong on the first connection").await;

        // No disconnect envelope: the server just drops the socket.
        conn1
            .send(ServerCmd::Close)
            .expect("the scripted connection is still open");

        let conn2 = harness
            .connected("the reconnect after the abrupt close")
            .await;
        conn2
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn2
            .send(ServerCmd::Ping)
            .expect("the scripted connection is still open");
        harness.pong("the pong on the reconnected socket").await;

        harness.client.abort();
    }

    /// Liveness (Task 4 review I-1): with no traffic at all, the client
    /// still pings on its keepalive cadence. Paused clock with a quiet fake:
    /// the runtime jumps straight to each ping timer, so the 30 s cadence is
    /// observable without waiting a real minute — and the fake's pong
    /// answers (axum auto-pongs at the protocol layer) refresh the silence
    /// deadline, proving the timeout does not fire on a healthy connection.
    #[tokio::test(start_paused = true)]
    async fn the_client_pings_on_the_keepalive_cadence_when_the_connection_is_quiet() {
        let fake = spawn_fake_slack().await;
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        let conn1 = harness.connected("the socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        // Nothing further is scripted: the client is alone with its timers.
        harness
            .client_ping("the first keepalive ping after 30 quiet seconds")
            .await;
        harness
            .client_ping("the second keepalive ping on the cadence")
            .await;

        harness.client.abort();
        let log = &harness.log;
        let connections = log
            .iter()
            .filter(|observed| matches!(observed, Observed::Connected { .. }))
            .count();
        assert_eq!(
            connections, 1,
            "keepalive pings on a healthy connection must not trip a reconnect: {log:?}"
        );
    }

    /// Backoff schedule only, on a paused clock with no real I/O (the house
    /// rule: a paused runtime with nothing to do jumps to its next timer, so
    /// socket waits there fire timeouts at once). Every open returns a URL
    /// that fails WebSocket request building instantly, so the loop is pure
    /// open-fail → sleep → open-fail. The 60s cap sits past the wait
    /// toolkit's 60s hang guard, so it is proven on `grow_backoff` below,
    /// not here.
    #[tokio::test(start_paused = true)]
    async fn failed_opens_back_off_exponentially() {
        let api = ScriptedApi::scripted("not a websocket url".to_string(), Vec::new());
        let client = tokio::spawn(run_socket(api.clone(), |_envelope| {}));

        wait::until("six open attempts", || {
            std::future::ready((api.opens().len() >= 6).then_some(()))
        })
        .await;

        client.abort();
        let opens = api.opens();
        assert_eq!(
            opens.len(),
            6,
            "one fresh open_connection per attempt: {opens:?}"
        );
        let gaps: Vec<Duration> = opens
            .iter()
            .zip(opens.iter().skip(1))
            .map(|(earlier, later)| *later - *earlier)
            .collect();
        assert_eq!(
            gaps,
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
            ],
            "backoff doubles from 1s on every consecutive failure"
        );
    }

    /// One step in the in-memory ordering log: a frame the sink was asked to
    /// write, or an envelope handed to the dispatch callback.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Step {
        Sent(String),
        Dispatched(String),
    }

    fn parsed(fixture: &str) -> Envelope {
        serde_json::from_str(fixture).expect("fixture parses")
    }

    /// Ack-first, deterministically: with a sink and a callback that write
    /// to one shared log, the ack frame is written strictly before the
    /// envelope is dispatched — no network, no race in the observation.
    #[tokio::test]
    async fn acks_on_the_socket_before_dispatching() {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_log = Arc::clone(&log);
        let mut sink = Box::pin(futures_util::sink::unfold((), move |(), frame: Message| {
            let sink_log = Arc::clone(&sink_log);
            async move {
                let text = frame
                    .into_text()
                    .expect("the ack is a text frame")
                    .to_string();
                sink_log.lock().expect("log lock").push(Step::Sent(text));
                Ok::<(), std::convert::Infallible>(())
            }
        }));
        let dispatch_log = Arc::clone(&log);
        let mut on_envelope = move |envelope: Envelope| {
            dispatch_log
                .lock()
                .expect("log lock")
                .push(Step::Dispatched(envelope.id));
        };

        let events = parsed(&events_envelope("env-order-1"));
        ack_then_dispatch(&mut sink, events, &mut on_envelope).await;
        let unknown = parsed(&unknown_envelope("env-order-unknown"));
        ack_then_dispatch(&mut sink, unknown, &mut on_envelope).await;

        let steps = log.lock().expect("log lock").clone();
        assert_eq!(
            steps,
            [
                Step::Sent(r#"{"envelope_id":"env-order-1"}"#.to_string()),
                Step::Dispatched("env-order-1".to_string()),
                Step::Sent(r#"{"envelope_id":"env-order-unknown"}"#.to_string()),
            ],
            "ack frame first, then dispatch; unknown types are acked but never dispatched"
        );
    }

    /// Envelopes without an `envelope_id` (hello, disconnect) are never
    /// acked, and a failing ack send never blocks dispatch.
    #[tokio::test]
    async fn id_less_envelopes_skip_the_ack_and_a_failed_ack_still_dispatches() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink_attempts = Arc::clone(&attempts);
        let mut failing = Box::pin(futures_util::sink::unfold(
            (),
            move |(), _frame: Message| {
                sink_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Err::<(), _>("the write side is gone".to_string()) }
            },
        ));
        let mut dispatched = Vec::new();

        let hello = parsed(&hello_text());
        ack(&mut failing, &hello).await;
        let disconnect = parsed(&disconnect_text());
        ack(&mut failing, &disconnect).await;
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "hello and disconnect carry no envelope_id and are never acked"
        );

        let events = parsed(&events_envelope("env-deadack"));
        ack_then_dispatch(&mut failing, events, &mut |envelope: Envelope| {
            dispatched.push(envelope.id);
        })
        .await;
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the ack was attempted"
        );
        assert_eq!(
            dispatched,
            ["env-deadack"],
            "a failed ack never blocks dispatch"
        );
    }

    #[test]
    fn grow_backoff_caps_at_sixty_seconds() {
        let mut backoff = BACKOFF_START;
        let mut schedule = Vec::new();
        for _ in 0..3 {
            backoff = grow_backoff(backoff);
            schedule.push(backoff);
        }
        assert_eq!(
            schedule,
            [
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ],
            "doubling continues"
        );
        assert_eq!(
            grow_backoff(Duration::from_secs(32)),
            Duration::from_secs(60)
        );
        assert_eq!(
            grow_backoff(Duration::from_secs(60)),
            Duration::from_secs(60)
        );
        assert_eq!(
            grow_backoff(BACKOFF_MAX),
            BACKOFF_MAX,
            "the cap holds forever"
        );
    }
}
