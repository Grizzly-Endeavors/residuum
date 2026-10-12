//! Slack Socket Mode client — connect, read envelopes, ack-first, reconnect.
//!
//! Owns one WebSocket per Slack connection: every cycle starts with a fresh
//! `apps.connections.open` (the wss URL rotates between connections), reads
//! `Message::Text` frames as [`Envelope`]s, and acknowledges *before*
//! dispatching — Slack redelivers un-acked envelopes and upstream dedup
//! absorbs the overlap (design doc, Lifecycle §4). A `disconnect` envelope
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
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::api::SlackApi;
use super::events::{Envelope, EnvelopeKind};

/// Reconnect backoff after a failed connect or read cycle; it doubles from
/// here on each consecutive failure.
const BACKOFF_START: Duration = Duration::from_secs(1);
/// Ceiling the reconnect backoff doubles up to, but never past.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Budget for one ack POST. Slack answers acks in well under a second; a
/// stuck one must not stall envelope processing for good.
const ACK_TIMEOUT: Duration = Duration::from_secs(15);

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
/// ack POST has been attempted (ack-first). `hello` and `disconnect` are
/// connection lifecycle, not conversation: handled here, never dispatched.
/// Unknown envelope types are logged, acked when they carry a top-level
/// `response_url`, and skipped (design D4).
///
/// # Errors
/// Only when the ack HTTP client cannot be built (TLS setup). Every connect
/// or read failure reconnects forever with capped backoff instead of
/// surfacing.
pub(crate) async fn run_socket<A, F>(api: A, mut on_envelope: F) -> anyhow::Result<()>
where
    A: SlackApi + 'static,
    F: FnMut(Envelope) + Send + 'static,
{
    let ack_http = reqwest::Client::builder()
        .timeout(ACK_TIMEOUT)
        .build()
        .context("failed to build the slack ack client")?;

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
            &ack_http,
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
/// `events_api` and unknown envelope before it is dispatched or skipped.
///
/// Liveness runs alongside the reads (see [`KEEPALIVE_PING_INTERVAL`]): the
/// socket is split so keepalive pings and pongs can be written while a read
/// is pending, and the read-silence timeout — refreshed by every inbound
/// frame — returns [`Cycle::Lost`] for a connection that has gone quiet,
/// feeding the caller's reconnect path exactly like a read error.
async fn read_cycle<F>(
    socket: SlackSocket,
    ack_http: &reqwest::Client,
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
                            // Ack-first (design, Lifecycle §4): Slack redelivers
                            // un-acked envelopes; upstream dedup absorbs the overlap.
                            EnvelopeKind::EventsApi => {
                                ack(ack_http, &envelope).await;
                                on_envelope(envelope);
                            }
                            EnvelopeKind::Unknown(kind) => {
                                tracing::debug!(
                                    envelope_type = %kind,
                                    "slack socket: unknown envelope type — acked when it carries a response_url, then skipped"
                                );
                                ack(ack_http, &envelope).await;
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

/// POST `{}` to the envelope's ack URL — before any dispatch.
///
/// RULING (from the fixture review): ack only when the top-level
/// `response_url` is present and non-empty; unknown/interactive envelopes
/// may carry one inside the payload or not at all, so never assume. Ack
/// failures warn and move on: Slack redelivers, upstream dedup absorbs it
/// (design, Lifecycle §4).
async fn ack(http: &reqwest::Client, envelope: &Envelope) {
    if envelope.response_url.is_empty() {
        tracing::debug!(
            envelope_id = %envelope.id,
            "slack socket: no response_url on the envelope — skipping the ack"
        );
        return;
    }
    match http
        .post(&envelope.response_url)
        .json(&serde_json::json!({}))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::warn!(
                envelope_id = %envelope.id,
                status = %response.status(),
                "slack socket: ack answered non-success — Slack may redeliver; upstream dedup absorbs it"
            );
        }
        Err(error) => {
            tracing::warn!(
                envelope_id = %envelope.id,
                error = %error,
                "slack socket: ack failed — Slack will redeliver; upstream dedup absorbs it"
            );
        }
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
    use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
    use axum::extract::{Path, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use crate::interfaces::slack::api::SlackApiError;
    use crate::testing::wait;

    /// What the driver observes: connections accepted, ack POSTs received,
    /// envelopes dispatched to the callback. One channel so the *order*
    /// between them is provable.
    #[derive(Debug, Clone)]
    enum Observed {
        /// A ws connection was accepted; the driver steers it through `cmds`.
        Connected {
            cmds: mpsc::UnboundedSender<ServerCmd>,
        },
        /// An ack POST landed on the fake server's `/ack/{id}` route.
        Ack { id: String, body: String },
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
    /// records ack POSTs. Real fixture texts are used as envelope templates
    /// with the ack pointed at this server's own POST route.
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
                            Some(Ok(WsMessage::Text(_) | WsMessage::Binary(_))) => {}
                        }
                    }
                }
            }
        })
    }

    async fn ack_route(
        State(shared): State<FakeSlackShared>,
        Path(id): Path<String>,
        body: String,
    ) -> &'static str {
        shared
            .obs
            .send(Observed::Ack { id, body })
            .expect("the driver is still observing");
        "ok"
    }

    /// A running fake Slack on a loopback port.
    struct FakeSlack {
        /// What the fake `SlackApi::open_connection` returns.
        ws_url: String,
        /// Base for `response_url`s the scripted envelopes carry.
        http_base: String,
        obs_tx: mpsc::UnboundedSender<Observed>,
        obs_rx: mpsc::UnboundedReceiver<Observed>,
    }

    async fn spawn_fake_slack() -> FakeSlack {
        let (obs_tx, obs_rx) = mpsc::unbounded_channel();
        let router = Router::new()
            .route("/ws", get(slack_socket_route))
            .route("/ack/{id}", post(ack_route))
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
            http_base: format!("http://{addr}"),
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
                Observed::Ack { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong
                | Observed::ClientPing => panic!(
                    "expected a socket connection while waiting for {what}, saw {observed:?}"
                ),
            }
        }

        async fn ack_post(&mut self, what: &str) -> (String, String) {
            let observed = self.next(what).await;
            match observed {
                Observed::Ack { id, body } => (id, body),
                Observed::Connected { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong
                | Observed::ClientPing => {
                    panic!("expected an ack POST while waiting for {what}, saw {observed:?}")
                }
            }
        }

        async fn dispatched(&mut self, what: &str) -> String {
            let observed = self.next(what).await;
            match observed {
                Observed::Dispatch { id } => id,
                Observed::Connected { .. }
                | Observed::Ack { .. }
                | Observed::Pong
                | Observed::ClientPing => panic!(
                    "expected an envelope dispatch while waiting for {what}, saw {observed:?}"
                ),
            }
        }

        async fn pong(&mut self, what: &str) {
            let observed = self.next(what).await;
            match observed {
                Observed::Pong => {}
                Observed::Connected { .. }
                | Observed::Ack { .. }
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
                | Observed::Ack { .. }
                | Observed::Dispatch { .. }
                | Observed::Pong => {
                    panic!(
                        "expected a client keepalive ping while waiting for {what}, saw {observed:?}"
                    );
                }
            }
        }
    }

    /// Index of the first observation matching `pred`, for order proofs.
    fn position_of(log: &[Observed], pred: impl Fn(&Observed) -> bool) -> Option<usize> {
        log.iter().position(pred)
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
        async fn post_message(&self, _channel: &str, _text: &str) -> Result<String, SlackApiError> {
            panic!("run_socket never posts messages; acks ride their own client");
        }

        async fn chat_update(
            &self,
            _channel: &str,
            _ts: &str,
            _text: &str,
        ) -> Result<(), SlackApiError> {
            panic!("run_socket never edits messages; acks ride their own client");
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

    /// The hello fixture text — no `response_url`, no `envelope_id`.
    fn hello_text() -> String {
        include_str!("fixtures/envelope-hello.json").to_string()
    }

    /// The corrected disconnect fixture — `{"type":"disconnect",…}` with no
    /// `envelope_id` and no payload.
    fn disconnect_text() -> String {
        include_str!("fixtures/envelope-disconnect.json").to_string()
    }

    /// The `events_api` fixture with its identity and ack destination swapped:
    /// the fake server controls `response_url`, so each envelope acks to the
    /// server's own POST route. `None` drops the field entirely.
    fn events_envelope(id: &str, response_url: Option<&str>) -> String {
        rewire(
            include_str!("fixtures/envelope-events-api-message.json"),
            id,
            response_url,
        )
    }

    /// The unknown-type fixture, likewise rewired. Fixture review ruling: a
    /// `response_url` may sit at the top level, inside the payload, or nowhere
    /// — the client acks only on the top-level field, never assumes.
    fn unknown_envelope(id: &str, response_url: Option<&str>) -> String {
        rewire(
            include_str!("fixtures/envelope-unknown.json"),
            id,
            response_url,
        )
    }

    fn rewire(fixture: &str, id: &str, response_url: Option<&str>) -> String {
        let mut envelope: serde_json::Value =
            serde_json::from_str(fixture).expect("fixture parses");
        let object = envelope
            .as_object_mut()
            .expect("every envelope fixture is a JSON object");
        object.insert(
            "envelope_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
        match response_url {
            Some(url) => {
                object.insert(
                    "response_url".to_string(),
                    serde_json::Value::String(url.to_string()),
                );
            }
            None => {
                object.remove("response_url");
            }
        }
        serde_json::to_string(&envelope).expect("rewired fixture serializes")
    }

    #[tokio::test]
    async fn acks_before_dispatch_then_reconnects_on_disconnect() {
        let fake = spawn_fake_slack().await;
        let http_base = fake.http_base.clone();
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        // Connection one: hello, then a DM event that must be acked first.
        let conn1 = harness.connected("the first socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn1
            .send(ServerCmd::Text(events_envelope(
                "env-evt-1",
                Some(&format!("{http_base}/ack/env-evt-1")),
            )))
            .expect("the scripted connection is still open");

        let (ack_id, ack_body) = harness.ack_post("the ack POST for env-evt-1").await;
        assert_eq!(ack_id, "env-evt-1");
        assert_eq!(ack_body, "{}", "the ack body is the empty object");
        let dispatched = harness.dispatched("the dispatch of env-evt-1").await;
        assert_eq!(dispatched, "env-evt-1");

        // Graceful disconnect → immediate reconnect → hello again.
        conn1
            .send(ServerCmd::Text(disconnect_text()))
            .expect("the scripted connection is still open");
        let conn2 = harness.connected("the reconnect after disconnect").await;
        conn2
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn2
            .send(ServerCmd::Text(events_envelope(
                "env-evt-2",
                Some(&format!("{http_base}/ack/env-evt-2")),
            )))
            .expect("the scripted connection is still open");
        let (ack_two, _) = harness.ack_post("the ack POST for env-evt-2").await;
        assert_eq!(ack_two, "env-evt-2");
        let dispatched_two = harness.dispatched("the dispatch of env-evt-2").await;
        assert_eq!(dispatched_two, "env-evt-2");

        harness.client.abort();

        let log = &harness.log;
        let connections = log
            .iter()
            .filter(|observed| matches!(observed, Observed::Connected { .. }))
            .count();
        assert_eq!(connections, 2, "both connections were seen: {log:?}");
        for id in ["env-evt-1", "env-evt-2"] {
            let ack_at = position_of(
                log,
                |observed| matches!(observed, Observed::Ack { id: seen, .. } if seen == id),
            )
            .unwrap_or_else(|| panic!("no ack POST for {id} observed: {log:?}"));
            let dispatch_at = position_of(
                log,
                |observed| matches!(observed, Observed::Dispatch { id: seen } if seen == id),
            )
            .unwrap_or_else(|| panic!("no dispatch for {id} observed: {log:?}"));
            assert!(
                ack_at < dispatch_at,
                "ack-first: the ack POST for {id} must land before its dispatch: {log:?}"
            );
        }
        assert!(
            !log.iter().any(|observed| matches!(
                observed,
                Observed::Dispatch { id } if id.is_empty()
            )),
            "hello/disconnect carry no envelope_id and must never be dispatched: {log:?}"
        );
    }

    #[tokio::test]
    async fn unknown_envelopes_ack_only_with_a_response_url_and_empty_urls_skip_the_ack() {
        let fake = spawn_fake_slack().await;
        let http_base = fake.http_base.clone();
        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));

        let conn1 = harness.connected("the socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");

        // Unknown type carrying a top-level response_url: acked, not dispatched.
        conn1
            .send(ServerCmd::Text(unknown_envelope(
                "env-unknown-1",
                Some(&format!("{http_base}/ack/env-unknown-1")),
            )))
            .expect("the scripted connection is still open");
        let (ack_id, ack_body) = harness.ack_post("the ack for the unknown envelope").await;
        assert_eq!(ack_id, "env-unknown-1");
        assert_eq!(ack_body, "{}");

        // events_api without any response_url: dispatched, ack log-skipped.
        conn1
            .send(ServerCmd::Text(events_envelope("env-evt-nourl", None)))
            .expect("the scripted connection is still open");
        let dispatched = harness
            .dispatched("dispatch of the url-less envelope")
            .await;
        assert_eq!(dispatched, "env-evt-nourl");

        harness.client.abort();

        let log = &harness.log;
        let acks = log
            .iter()
            .filter(|observed| matches!(observed, Observed::Ack { .. }))
            .count();
        assert_eq!(
            acks, 1,
            "exactly one ack — the url-less envelope skips it: {log:?}"
        );
        assert!(
            !log.iter().any(|observed| matches!(
                observed,
                Observed::Dispatch { id: seen } if seen == "env-unknown-1"
            )),
            "unknown envelope types are acked and skipped, never dispatched: {log:?}"
        );
    }

    #[tokio::test]
    async fn failed_acks_do_not_block_dispatch() {
        let fake = spawn_fake_slack().await;
        // A loopback port that nothing will ever listen on: the ack POST is
        // refused instantly instead of hanging.
        let closed = TcpListener::bind("127.0.0.1:0").await.expect("bind probe");
        let dead_port = closed.local_addr().expect("probe address").port();
        drop(closed);

        let ws_url = fake.ws_url.clone();
        let mut harness = fake.drive(ScriptedApi::live(ws_url));
        let conn1 = harness.connected("the socket connection").await;
        conn1
            .send(ServerCmd::Text(hello_text()))
            .expect("the scripted connection is still open");
        conn1
            .send(ServerCmd::Text(events_envelope(
                "env-evt-deadack",
                Some(&format!("http://127.0.0.1:{dead_port}/ack/env-evt-deadack")),
            )))
            .expect("the scripted connection is still open");

        let dispatched = harness
            .dispatched("dispatch despite the dead ack destination")
            .await;
        assert_eq!(dispatched, "env-evt-deadack");

        harness.client.abort();
        assert!(
            !harness
                .log
                .iter()
                .any(|observed| matches!(observed, Observed::Ack { .. })),
            "no ack can arrive from a dead destination: {:?}",
            harness.log
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

    #[tokio::test]
    async fn a_malformed_text_frame_is_skipped_and_the_connection_stays_up() {
        let fake = spawn_fake_slack().await;
        let http_base = fake.http_base.clone();
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
            .send(ServerCmd::Text(events_envelope(
                "env-after-garbage",
                Some(&format!("{http_base}/ack/env-after-garbage")),
            )))
            .expect("the scripted connection is still open");

        let (ack_id, _) = harness
            .ack_post("the ack for the envelope after the garbage frame")
            .await;
        assert_eq!(ack_id, "env-after-garbage");
        let dispatched = harness
            .dispatched("the dispatch of the envelope after the garbage frame")
            .await;
        assert_eq!(dispatched, "env-after-garbage");

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
