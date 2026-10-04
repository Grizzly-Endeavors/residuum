//! How a forwarded socket's loopback connection ends: closed by the relay,
//! closed by the local listener, or lost with the tunnel.
//!
//! The tests run the real frame loop against a real local listener that sees
//! each connection the way the hub does: whether a Close frame arrived, and
//! whether the tunnel then let go of its end of the TCP connection. A
//! connection the tunnel leaves open is the leak these tests exist to catch:
//! the hub keeps pushing frames at it, the agent keeps counting a client, and
//! the relay is told about messages for a channel it already closed.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::AsyncReadExt as _;
use tokio_tungstenite::tungstenite::Error as WsError;

use super::*;
use crate::hub::test_support::EventLog;

/// The text a client sends to make the local listener close its socket.
const CLOSE_COMMAND: &str = "close";

/// How long the local listener waits for the tunnel to let go of a
/// connection after the socket's closing handshake, before it reports that
/// the tunnel didn't.
const TCP_CLOSE_WAIT: Duration = Duration::from_secs(3);

/// How long a test waits to see that something does not happen.
const QUIET_WINDOW: Duration = Duration::from_millis(300);

/// How long the frame loop gets to act on an event no test frame follows.
const SETTLE: Duration = Duration::from_millis(100);

/// The text of the debug line a closed channel logs.
const CLOSED_LINE: &str = "local WebSocket channel closed";

/// How one connection to the local listener ended.
#[derive(Debug)]
struct LocalEnd {
    /// The peer sent a Close frame before the connection ended.
    saw_close_frame: bool,
    /// The peer then closed its end of the TCP connection.
    peer_closed_tcp: bool,
}

/// Counts a connection from accept until the listener is done with it.
struct LiveConnection(Arc<AtomicUsize>);

impl Drop for LiveConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A local WebSocket listener that echoes text, closes its socket on
/// [`CLOSE_COMMAND`], and reports how each connection ended.
struct LocalListener {
    port: u16,
    live: Arc<AtomicUsize>,
    ended: mpsc::UnboundedReceiver<LocalEnd>,
}

async fn local_listener() -> LocalListener {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let live = Arc::new(AtomicUsize::new(0));
    let (ended_tx, ended) = mpsc::unbounded_channel();
    let accepted = Arc::clone(&live);
    crate::util::spawn_in_span(async move {
        while let Ok((stream, _)) = listener.accept().await {
            accepted.fetch_add(1, Ordering::SeqCst);
            let connection = LiveConnection(Arc::clone(&accepted));
            crate::util::spawn_in_span(serve_connection(stream, connection, ended_tx.clone()));
        }
    });
    LocalListener { port, live, ended }
}

async fn serve_connection(
    stream: TcpStream,
    _connection: LiveConnection,
    ended: mpsc::UnboundedSender<LocalEnd>,
) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let mut saw_close_frame = false;
    while let Some(Ok(frame)) = socket.next().await {
        match frame {
            Message::Text(text) if text.as_str() == CLOSE_COMMAND => {
                socket.close(None).await.ok();
            }
            Message::Text(text) => {
                socket
                    .send(Message::Text(format!("echo {text}").into()))
                    .await
                    .ok();
            }
            Message::Close(_) => saw_close_frame = true,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    // The closing handshake is over. A peer that keeps its end of the
    // connection open is still connected, whatever the socket protocol says.
    let mut scratch = [0_u8; 64];
    let peer_closed_tcp = tokio::time::timeout(TCP_CLOSE_WAIT, async {
        while let Ok(read) = socket.get_mut().read(&mut scratch).await {
            if read == 0 {
                break;
            }
        }
    })
    .await
    .is_ok();
    ended
        .send(LocalEnd {
            saw_close_frame,
            peer_closed_tcp,
        })
        .ok();
}

impl LocalListener {
    fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// How the next connection to end ended. A connection the tunnel never
    /// closes never ends, so this waits out [`TEST_TIMEOUT`] and fails.
    async fn next_end(&mut self) -> LocalEnd {
        tokio::time::timeout(TEST_TIMEOUT, self.ended.recv())
            .await
            .expect("the local listener never saw its connection end: the tunnel left it open")
            .expect("the listener outlives its connections")
    }

    /// Waits for the listener to let go of every connection it accepted.
    async fn expect_no_connections(&self) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            while self.live() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a connection to the local listener outlived its channel");
    }
}

/// The tasks alive on this test's runtime.
fn alive_tasks() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

/// Waits for the runtime's task count to come back to `baseline`, the count
/// before a channel opened: once a channel has ended, none of its tasks may
/// be left behind.
async fn expect_tasks_back_to(baseline: usize) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while alive_tasks() > baseline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .ok();
    assert!(
        alive_tasks() <= baseline,
        "a closed channel left tasks running: {} alive, {baseline} before it opened",
        alive_tasks()
    );
}

/// The real frame loop, with a scripted relay on one side and a real local
/// listener on the other.
struct ChannelHarness {
    /// The relay's end of the tunnel: what the tunnel client sends.
    relay: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    /// Frames the relay sends the tunnel client; dropping it ends the tunnel
    /// connection.
    from_relay: mpsc::UnboundedSender<Result<Message, WsError>>,
    shutdown_tx: watch::Sender<bool>,
    _agents_tx: watch::Sender<Vec<AgentInfo>>,
    task: tokio::task::JoinHandle<LoopExit>,
    local: LocalListener,
    /// The runtime's task count with the loop idle and no channel open.
    baseline: usize,
}

impl ChannelHarness {
    async fn start() -> Self {
        let local = local_listener().await;
        let (write, relay) = loopback_ws().await;
        let (from_relay, mut from_relay_rx) = mpsc::unbounded_channel();
        let (agents_tx, mut agents_rx) = watch::channel(Vec::new());
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let targets = ForwardTargets {
            main: local.port,
            workbench: None,
            a2a: None,
        };
        let task = crate::util::spawn_in_span(async move {
            let client = forward_http::forwarding_client().unwrap();
            let a2a_client = forward_a2a::forwarding_client().unwrap();
            let (status_tx, _status_rx) = watch::channel(TunnelStatus::Disconnected);
            let mut read = futures_util::stream::poll_fn(move |cx| from_relay_rx.poll_recv(cx));
            let (_teams_tx, teams_ports) = watch::channel(BTreeMap::new());
            run_tunnel_loop(
                LoopContext {
                    clients: TunnelClients {
                        client: &client,
                        a2a_client: &a2a_client,
                        teams_ports: &teams_ports,
                    },
                    targets,
                    keepalive_timeout: Duration::from_secs(60),
                    agents_rx: &mut agents_rx,
                },
                &mut read,
                &write,
                &mut shutdown_rx,
                &status_tx,
            )
            .await
        });
        // Let the loop reach its idle state before the task count is taken.
        tokio::time::sleep(SETTLE).await;
        let baseline = alive_tasks();
        Self {
            relay,
            from_relay,
            shutdown_tx,
            _agents_tx: agents_tx,
            task,
            local,
            baseline,
        }
    }

    /// Delivers `frame` to the tunnel as the relay would.
    fn relay_sends(&self, frame: &TunnelFrame) {
        let text = serde_json::to_string(frame).unwrap();
        self.from_relay
            .send(Ok(Message::Text(text.into())))
            .expect("the frame loop is reading");
    }

    /// Opens a socket channel to the local listener, and returns once the
    /// loop has registered it: a message sent to the channel reaches the
    /// listener and its echo comes back.
    async fn open(&mut self, channel_id: &str) {
        self.relay_sends(&TunnelFrame::WsOpen {
            channel_id: channel_id.to_string(),
            path: "/api/hub/ws".to_string(),
            headers: HashMap::new(),
            surface: None,
        });
        let answer = recv_ws_frame(&mut self.relay).await;
        assert!(
            matches!(answer, TunnelFrame::WsOpenResult { success: true, .. }),
            "the open must succeed: {answer:?}"
        );
        self.relay_sends(&TunnelFrame::WsMessage {
            channel_id: channel_id.to_string(),
            data: "hello".to_string(),
        });
        let echo = recv_ws_frame(&mut self.relay).await;
        assert!(
            matches!(&echo, TunnelFrame::WsMessage { data, .. } if data == "echo hello"),
            "the listener's echo must come back through the channel: {echo:?}"
        );
    }

    /// Asserts the tunnel sends the relay nothing for a while.
    async fn expect_relay_quiet(&mut self) {
        let frame = tokio::time::timeout(QUIET_WINDOW, self.relay.next()).await;
        assert!(frame.is_err(), "the tunnel sent the relay {frame:?}");
    }

    /// Ends the tunnel connection the way a relay disconnect does and returns
    /// how many channels the loop still counted as open.
    async fn drop_the_tunnel(self) -> usize {
        drop(self.from_relay);
        let exit = with_timeout(self.task).await.unwrap();
        let LoopExit::Reconnect(_, open_channels) = exit else {
            panic!("a dropped tunnel reconnects");
        };
        open_channels
    }
}

/// Every event at `warn` or above.
fn warnings(log: &EventLog) -> Vec<crate::hub::test_support::LoggedEvent> {
    log.events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.level <= tracing::Level::WARN)
        .cloned()
        .collect()
}

/// A normal close logs one debug line for the channel and nothing at `warn`.
fn assert_logged_one_normal_close(log: &EventLog, closed_by: &str) {
    let closes = log.matching(CLOSED_LINE);
    let [close] = closes.as_slice() else {
        panic!("one line per closed channel: {closes:?}");
    };
    assert_eq!(close.level, tracing::Level::DEBUG);
    assert!(
        close.text.contains(&format!("closed_by={closed_by:?}")),
        "the line says who closed the channel: {close:?}"
    );
    let noisy = warnings(log);
    assert!(
        noisy.is_empty(),
        "a normal close warns of nothing: {noisy:?}"
    );
}

#[tokio::test]
async fn a_relay_close_closes_the_local_connection() {
    let log = EventLog::default();
    let _capture = log.capture();
    let mut h = ChannelHarness::start().await;
    h.open("ch-1").await;
    assert_eq!(h.local.live(), 1);

    h.relay_sends(&TunnelFrame::WsClose {
        channel_id: "ch-1".to_string(),
    });

    let end = h.local.next_end().await;
    assert!(
        end.saw_close_frame,
        "the local listener must be told the socket is closing"
    );
    assert!(
        end.peer_closed_tcp,
        "the tunnel must let go of the loopback connection"
    );
    h.local.expect_no_connections().await;
    expect_tasks_back_to(h.baseline).await;
    h.expect_relay_quiet().await;
    assert_eq!(
        h.drop_the_tunnel().await,
        0,
        "the closed channel must not be counted as open"
    );
    assert_logged_one_normal_close(&log, "tunnel");
}

#[tokio::test]
async fn a_local_close_tells_the_relay_once() {
    let log = EventLog::default();
    let _capture = log.capture();
    let mut h = ChannelHarness::start().await;
    h.open("ch-1").await;

    h.relay_sends(&TunnelFrame::WsMessage {
        channel_id: "ch-1".to_string(),
        data: CLOSE_COMMAND.to_string(),
    });

    let frame = recv_ws_frame(&mut h.relay).await;
    assert!(
        matches!(&frame, TunnelFrame::WsClose { channel_id } if channel_id == "ch-1"),
        "the relay must be told the channel closed: {frame:?}"
    );
    h.expect_relay_quiet().await;
    let end = h.local.next_end().await;
    assert!(
        end.peer_closed_tcp,
        "the tunnel must let go of the loopback connection"
    );
    h.local.expect_no_connections().await;
    expect_tasks_back_to(h.baseline).await;
    // The relay never sends its own close back, so only the loop's own
    // bookkeeping can have dropped the channel.
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        h.drop_the_tunnel().await,
        0,
        "a channel the local listener closed must not be counted as open"
    );
    assert_logged_one_normal_close(&log, "local");
}

#[tokio::test]
async fn a_message_that_crosses_a_local_close_is_dropped_quietly() {
    let log = EventLog::default();
    let _capture = log.capture();
    let mut h = ChannelHarness::start().await;
    h.open("ch-1").await;
    h.relay_sends(&TunnelFrame::WsMessage {
        channel_id: "ch-1".to_string(),
        data: CLOSE_COMMAND.to_string(),
    });
    let frame = recv_ws_frame(&mut h.relay).await;
    assert!(matches!(frame, TunnelFrame::WsClose { .. }), "{frame:?}");
    h.local.expect_no_connections().await;

    // The relay sent these before it saw the close, and answers it with its
    // own close.
    h.relay_sends(&TunnelFrame::WsMessage {
        channel_id: "ch-1".to_string(),
        data: "late".to_string(),
    });
    h.relay_sends(&TunnelFrame::WsClose {
        channel_id: "ch-1".to_string(),
    });
    tokio::time::sleep(SETTLE).await;

    h.expect_relay_quiet().await;
    assert_eq!(h.drop_the_tunnel().await, 0);
    assert_logged_one_normal_close(&log, "local");
}

#[tokio::test]
async fn losing_the_tunnel_closes_every_local_connection() {
    let log = EventLog::default();
    let _capture = log.capture();
    let mut h = ChannelHarness::start().await;
    h.open("ch-1").await;
    h.open("ch-2").await;
    assert_eq!(h.local.live(), 2);
    let baseline = h.baseline;
    let mut local = h.local;
    let mut relay = h.relay;
    drop(h.from_relay);

    let exit = with_timeout(h.task).await.unwrap();
    let LoopExit::Reconnect(_, open_channels) = exit else {
        panic!("a dropped tunnel reconnects");
    };
    assert_eq!(open_channels, 2, "the loop reports what the drop closes");
    for _ in 0..2 {
        let end = local.next_end().await;
        assert!(end.saw_close_frame, "{end:?}");
        assert!(end.peer_closed_tcp, "{end:?}");
    }
    local.expect_no_connections().await;
    expect_tasks_back_to(baseline).await;

    // Nothing the tunnel sent after the drop may have been a close: the relay
    // forgets a tunnel's channels with the tunnel.
    let mut frames = Vec::new();
    with_timeout(async {
        while let Some(Ok(Message::Text(text))) = relay.next().await {
            frames.push(serde_json::from_str::<TunnelFrame>(&text).unwrap());
        }
    })
    .await;
    assert!(frames.is_empty(), "the relay was sent {frames:?}");
    assert_eq!(log.matching(CLOSED_LINE).len(), 2, "one line per channel");
    let noisy = warnings(&log);
    assert!(
        noisy.is_empty(),
        "losing the tunnel warns of nothing: {noisy:?}"
    );
}

#[tokio::test]
async fn shutting_the_tunnel_down_closes_every_local_connection() {
    let mut h = ChannelHarness::start().await;
    h.open("ch-1").await;
    h.open("ch-2").await;

    h.shutdown_tx.send(true).unwrap();

    let exit = with_timeout(h.task).await.unwrap();
    assert!(matches!(exit, LoopExit::Shutdown));
    for _ in 0..2 {
        let end = h.local.next_end().await;
        assert!(end.saw_close_frame, "{end:?}");
        assert!(end.peer_closed_tcp, "{end:?}");
    }
    h.local.expect_no_connections().await;
    expect_tasks_back_to(h.baseline).await;
}
