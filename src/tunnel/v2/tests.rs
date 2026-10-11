//! End-to-end tests of the v2 client against the fake relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use super::frames::{AgentInfo, V2Frame};
use super::test_relay::{FakeRelay, FakeRelayConfig, V2Mode, client_hello};
use super::{ClaimError, ConnectedInfo, IncomingStream, RelayLink, SessionHandler, Verdict};
use crate::config::{CloudConfig, RemoteAccessSettings};
use crate::testing::wait;
use crate::tunnel::TunnelStatus;

struct TestHandler {
    refuse: Option<String>,
    unsupported: AtomicUsize,
    streams: mpsc::UnboundedSender<IncomingStream>,
    infos: mpsc::UnboundedSender<ConnectedInfo>,
    links: mpsc::UnboundedSender<RelayLink>,
    disconnects: AtomicUsize,
}

struct HandlerOutputs {
    streams: mpsc::UnboundedReceiver<IncomingStream>,
    infos: mpsc::UnboundedReceiver<ConnectedInfo>,
    links: mpsc::UnboundedReceiver<RelayLink>,
}

fn handler(refuse: Option<&str>) -> (Arc<TestHandler>, HandlerOutputs) {
    let (streams_tx, streams) = mpsc::unbounded_channel();
    let (infos_tx, infos) = mpsc::unbounded_channel();
    let (links_tx, links) = mpsc::unbounded_channel();
    (
        Arc::new(TestHandler {
            refuse: refuse.map(str::to_string),
            unsupported: AtomicUsize::new(0),
            streams: streams_tx,
            infos: infos_tx,
            links: links_tx,
            disconnects: AtomicUsize::new(0),
        }),
        HandlerOutputs {
            streams,
            infos,
            links,
        },
    )
}

#[async_trait]
impl SessionHandler for TestHandler {
    async fn on_connected(&self, connected: &ConnectedInfo, link: RelayLink) -> Verdict {
        self.infos.send(connected.clone()).ok();
        self.links.send(link).ok();
        match &self.refuse {
            Some(reason) => Verdict::Refuse(reason.clone()),
            None => Verdict::Accept {
                user: connected.user.clone(),
                instance: connected.instance.clone(),
                ui_origin: Some(format!("https://{}", connected.hosts.ui)),
                workbench_origin: Some(format!("https://{}", connected.hosts.workbench)),
                instance_origin: Some(format!("https://{}", connected.hosts.instance)),
            },
        }
    }

    fn on_stream(&self, stream: IncomingStream) {
        self.streams.send(stream).ok();
    }

    fn on_disconnected(&self) {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
    }

    fn on_relay_unsupported(&self) {
        self.unsupported.fetch_add(1, Ordering::SeqCst);
    }
}

struct Running {
    status: watch::Receiver<TunnelStatus>,
    shutdown: watch::Sender<bool>,
    agents: watch::Sender<Vec<AgentInfo>>,
    task: tokio::task::JoinHandle<()>,
}

fn start(relay: &FakeRelay, handler: &Arc<TestHandler>) -> Running {
    let cfg = CloudConfig {
        relay_url: relay.ws_url(),
        token: "rst_test".to_string(),
        remote: RemoteAccessSettings::default(),
    };
    let (status_tx, status) = watch::channel(TunnelStatus::Disconnected);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (agents, agents_rx) = watch::channel(Vec::new());
    let handler: Arc<dyn SessionHandler> = Arc::clone(handler) as Arc<dyn SessionHandler>;
    let task = tokio::spawn(crate::tunnel::start_tunnel(
        cfg,
        agents_rx,
        shutdown_rx,
        Arc::new(status_tx),
        handler,
    ));
    Running {
        status,
        shutdown,
        agents,
        task,
    }
}

async fn connected(running: &mut Running) -> TunnelStatus {
    wait::watch_until("the tunnel to connect", &mut running.status, |s| {
        matches!(s, TunnelStatus::Connected { .. })
    })
    .await
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    wait::until_true(what, check).await;
}

async fn next_stream(outputs: &mut HandlerOutputs) -> IncomingStream {
    wait::guarded("a browser stream to arrive", outputs.streams.recv())
        .await
        .expect("handler closed")
}

async fn stop(running: Running) {
    running.shutdown.send(true).ok();
    wait::guarded("the tunnel to stop after shutdown", running.task)
        .await
        .expect("tunnel task panicked");
}

fn agent(name: &str) -> AgentInfo {
    AgentInfo {
        name: name.to_string(),
        display_name: name.to_string(),
        a2a_enabled: false,
        a2a_private: false,
    }
}

#[tokio::test]
async fn accepted_session_publishes_connected_and_sends_agents() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);

    let status = connected(&mut running).await;
    assert_eq!(
        status,
        TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: Some("https://bear.relay.test".to_string()),
            workbench_origin: Some("https://bear.workbench.relay.test".to_string()),
            instance: Some("laptop".to_string()),
            instance_origin: Some("https://laptop.bear.relay.test".to_string()),
        }
    );
    let info = outputs.infos.recv().await.unwrap();
    assert_eq!(info.hosts.instance, "laptop.bear.relay.test");
    assert_eq!(info.keepalive_interval_secs, 30);
    assert!(
        relay
            .capabilities_seen()
            .iter()
            .all(|c| c.split(',').any(|cap| cap == "tls-passthrough"))
    );

    relay
        .wait_frame(|f| matches!(f, V2Frame::AgentsUpdate { agents } if agents.is_empty()))
        .await;
    running.agents.send(vec![agent("scout")]).unwrap();
    relay
        .wait_frame(|f| matches!(f, V2Frame::AgentsUpdate { agents } if agents.len() == 1))
        .await;
    stop(running).await;
    assert_eq!(h.disconnects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ping_is_answered_with_pong() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, _outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    relay.send_frame(&V2Frame::Ping);
    relay.wait_frame(|f| matches!(f, V2Frame::Pong)).await;
    stop(running).await;
}

#[tokio::test]
async fn refuse_closes_and_backs_off_instead_of_reconnecting() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, _outputs) = handler(Some("wrong relay"));
    let cfg = CloudConfig {
        relay_url: relay.ws_url(),
        token: "rst_test".to_string(),
        remote: RemoteAccessSettings::default(),
    };
    let handler: Arc<dyn SessionHandler> = Arc::clone(&h) as Arc<dyn SessionHandler>;
    let (status_tx, status) = watch::channel(TunnelStatus::Connecting);
    let (_shutdown, mut shutdown_rx) = watch::channel(false);
    let (_agents, mut agents_rx) = watch::channel(Vec::new());

    // One attempt, run to its decision: a refused relay is not reconnected to
    // until the longest backoff has passed.
    let attempt = crate::tunnel::connection::Attempt {
        cfg: &cfg,
        handler: &handler,
    };
    let step = wait::guarded(
        "the refused attempt to end",
        Box::pin(attempt.run(&mut agents_rx, &mut shutdown_rx, &status_tx)),
    )
    .await;
    assert_eq!(
        step,
        crate::tunnel::connection::Step::Wait(Some(crate::tunnel::connection::MAX_BACKOFF))
    );
    assert_eq!(*status.borrow(), TunnelStatus::Disconnected);
    eventually("on_disconnected", || {
        h.disconnects.load(Ordering::SeqCst) == 1
    })
    .await;
    eventually("the relay to see the connection drop", || {
        relay.v2_connections() == 1 && relay.connected_instances() == 0
    })
    .await;
}

#[tokio::test]
async fn a_relay_without_the_endpoint_is_reported_and_retried() {
    for code in [404, 426] {
        let relay = FakeRelay::start(FakeRelayConfig {
            v2: V2Mode::Respond(code),
            ..FakeRelayConfig::default()
        })
        .await;
        let (h, _outputs) = handler(None);
        let running = start(&relay, &h);
        eventually("the handler to hear the relay is unsupported", || {
            h.unsupported.load(Ordering::SeqCst) >= 1
        })
        .await;
        assert!(relay.v2_attempts() >= 1, "status {code}");
        assert_eq!(relay.v2_connections(), 0);
        assert_eq!(*running.status.borrow(), TunnelStatus::Disconnected);
        stop(running).await;
    }
}

#[tokio::test]
async fn a_relay_url_the_client_cannot_register_at_never_connects() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, _outputs) = handler(None);
    // Starts at Connecting so the Disconnected the test waits for can only be
    // the first attempt giving up on its URL, which is the point the loop has
    // decided not to connect.
    let (status_tx, mut status) = watch::channel(TunnelStatus::Connecting);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (_agents, agents_rx) = watch::channel(Vec::new());
    let cfg = CloudConfig {
        relay_url: relay.ws_url().replace("/tunnel/v2/register", "/elsewhere"),
        token: "rst_test".to_string(),
        remote: RemoteAccessSettings::default(),
    };
    let handler: Arc<dyn SessionHandler> = h;
    let task = tokio::spawn(crate::tunnel::start_tunnel(
        cfg,
        agents_rx,
        shutdown_rx,
        Arc::new(status_tx),
        handler,
    ));
    wait::watch_until("the tunnel to give up on its URL", &mut status, |s| {
        *s == TunnelStatus::Disconnected
    })
    .await;
    assert_eq!(relay.v2_attempts(), 0);
    assert!(!matches!(*status.borrow(), TunnelStatus::Connected { .. }));
    shutdown.send(true).ok();
    wait::guarded("the tunnel to stop", task).await.unwrap();
}

#[tokio::test]
async fn the_earlier_registration_path_in_the_config_still_reaches_the_endpoint() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, _outputs) = handler(None);
    let (status_tx, mut status) = watch::channel(TunnelStatus::Disconnected);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (_agents, agents_rx) = watch::channel(Vec::new());
    let cfg = CloudConfig {
        relay_url: relay
            .ws_url()
            .replace("/tunnel/v2/register", "/tunnel/register"),
        token: "rst_test".to_string(),
        remote: RemoteAccessSettings::default(),
    };
    let handler: Arc<dyn SessionHandler> = h;
    let task = tokio::spawn(crate::tunnel::start_tunnel(
        cfg,
        agents_rx,
        shutdown_rx,
        Arc::new(status_tx),
        handler,
    ));
    wait::watch_until(
        "the tunnel to connect on the older path",
        &mut status,
        |s| matches!(s, TunnelStatus::Connected { .. }),
    )
    .await;
    shutdown.send(true).ok();
    wait::guarded("the tunnel to stop", task).await.unwrap();
}

#[tokio::test]
async fn grant_and_claim_round_trips() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let link = outputs.links.recv().await.unwrap();

    assert_eq!(
        link.request_grant("enroll").await,
        Ok("test.grant.jws".into())
    );
    let names = vec!["bear.relay.test".to_string()];
    assert_eq!(link.claim_challenge(&names).await, Ok(()));
    link.release_challenge(&names);
    relay
        .wait_frame(
            |f| matches!(f, V2Frame::ChallengeRelease { names: released } if released.len() == 1),
        )
        .await;
    stop(running).await;
}

#[tokio::test]
async fn busy_claims_and_refused_grants_report_why() {
    let relay = FakeRelay::start(FakeRelayConfig {
        claim_busy: true,
        grant: Err("already enrolled".to_string()),
        ..FakeRelayConfig::default()
    })
    .await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let link = outputs.links.recv().await.unwrap();
    assert_eq!(
        link.claim_challenge(&["bear.relay.test".to_string()]).await,
        Err(ClaimError::Busy)
    );
    assert_eq!(
        link.request_grant("enroll").await,
        Err("already enrolled".to_string())
    );
    stop(running).await;
}

#[tokio::test]
async fn link_fails_fast_once_the_session_ended() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let link = outputs.links.recv().await.unwrap();
    relay.drop_connection();
    eventually("the link to close", || link.is_closed()).await;
    assert_eq!(
        link.claim_challenge(&["bear.relay.test".to_string()]).await,
        Err(ClaimError::Closed)
    );
    assert!(link.request_grant("reset").await.is_err());
    stop(running).await;
}

#[tokio::test]
async fn relay_close_delivers_queued_data_then_eof() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;

    let id = relay.open_raw_stream("Bear.Relay.Test", "203.0.113.7");
    relay.send_stream_data(id, b"abc");
    relay.send_stream_data(id, b"def");
    relay.send_frame(&V2Frame::StreamClose {
        stream_id: id,
        reason: None,
    });
    let mut incoming = next_stream(&mut outputs).await;
    assert_eq!(incoming.peer_ip, "203.0.113.7");
    let mut all = Vec::new();
    wait::guarded(
        "the relay's queued data and EOF",
        incoming.io.read_to_end(&mut all),
    )
    .await
    .unwrap();
    assert_eq!(all, b"abcdef");
    stop(running).await;
}

#[tokio::test]
async fn dropping_the_io_sends_one_stream_close() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let id = relay.open_raw_stream("bear.relay.test", "1.2.3.4");
    let incoming = next_stream(&mut outputs).await;
    drop(incoming);
    relay
        .wait_frame(|f| matches!(f, V2Frame::StreamClose { stream_id, .. } if *stream_id == id))
        .await;
    stop(running).await;
}

#[tokio::test]
async fn data_for_an_unknown_stream_is_answered_with_a_close() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, _outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let ghost = Uuid::new_v4();
    relay.send_stream_data(ghost, b"hello");
    relay
        .wait_frame(|f| matches!(f, V2Frame::StreamClose { stream_id, .. } if *stream_id == ghost))
        .await;
    let ghost2 = Uuid::new_v4();
    relay.send_frame(&V2Frame::StreamCredit {
        stream_id: ghost2,
        bytes: 5,
    });
    relay
        .wait_frame(|f| matches!(f, V2Frame::StreamClose { stream_id, .. } if *stream_id == ghost2))
        .await;
    stop(running).await;
}

#[tokio::test]
async fn a_relay_that_exceeds_the_window_gets_the_stream_closed() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let id = relay.open_raw_stream("bear.relay.test", "1.2.3.4");
    let mut incoming = next_stream(&mut outputs).await;
    for _ in 0..5 {
        relay.send_stream_data(id, &vec![1; 64 * 1024]);
    }
    relay
        .wait_frame(|f| {
            matches!(f, V2Frame::StreamClose { stream_id, reason: Some(r) }
                if *stream_id == id && r.contains("window"))
        })
        .await;
    assert!(incoming.io.read_u8().await.is_err());
    stop(running).await;
}

#[tokio::test]
async fn oversized_data_messages_close_the_stream() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let id = relay.open_raw_stream("bear.relay.test", "1.2.3.4");
    let _incoming = next_stream(&mut outputs).await;
    relay.send_stream_data(id, &vec![1; 64 * 1024 + 1]);
    relay
        .wait_frame(|f| {
            matches!(f, V2Frame::StreamClose { stream_id, reason: Some(r) }
                if *stream_id == id && r.contains("64 KiB"))
        })
        .await;
    stop(running).await;
}

#[tokio::test]
async fn the_257th_stream_is_refused() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let mut held = Vec::new();
    for _ in 0..256 {
        relay.open_raw_stream("bear.relay.test", "1.2.3.4");
        held.push(next_stream(&mut outputs).await);
    }
    let extra = relay.open_raw_stream("bear.relay.test", "1.2.3.4");
    relay
        .wait_frame(|f| matches!(f, V2Frame::StreamClose { stream_id, .. } if *stream_id == extra))
        .await;
    stop(running).await;
}

fn tls_material() -> (Arc<rustls::ServerConfig>, Arc<rustls::ClientConfig>) {
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
        "bear.relay.test".to_string(),
        "laptop.bear.relay.test".to_string(),
    ])
    .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let server = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key)
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (Arc::new(server), Arc::new(client))
}

async fn tls_client_exchange(
    port: u16,
    host: &'static str,
    config: Arc<rustls::ClientConfig>,
    request: Vec<u8>,
) -> Vec<u8> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from(host).unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(config)
        .connect(name, tcp)
        .await
        .unwrap();
    tls.write_all(&request).await.unwrap();
    tls.flush().await.unwrap();
    let mut reply = Vec::new();
    tls.read_to_end(&mut reply).await.ok();
    reply
}

/// Serve one TLS connection on `io`: read `expect` bytes, answer with
/// `reply`, close.
async fn tls_serve(
    server: Arc<rustls::ServerConfig>,
    io: super::TunnelIo,
    expect: usize,
    reply: Vec<u8>,
) {
    let mut tls = tokio_rustls::TlsAcceptor::from(server)
        .accept(io)
        .await
        .unwrap();
    let mut request = vec![0; expect];
    tls.read_exact(&mut request).await.unwrap();
    tls.write_all(&reply).await.unwrap();
    tls.shutdown().await.unwrap();
}

#[tokio::test]
async fn tls_hello_world_flows_through_the_framing() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let (server, client) = tls_material();

    let port = relay.front_door_port();
    let browser = tokio::spawn(tls_client_exchange(
        port,
        "bear.relay.test",
        client,
        b"hello relay".to_vec(),
    ));
    let incoming = next_stream(&mut outputs).await;
    assert_eq!(incoming.host, "bear.relay.test");
    assert_eq!(incoming.peer_ip, "127.0.0.1");
    // A reply bigger than one data message and the whole window crosses the
    // credit machinery in both directions.
    let reply: Vec<u8> = (0..600_000_u32).map(|i| (i % 251) as u8).collect();
    let server_task = tokio::spawn(tls_serve(server, incoming.io, 11, reply.clone()));
    let got = wait::guarded("the browser to read the full reply", browser)
        .await
        .unwrap();
    assert_eq!(got, reply);
    wait::guarded("the server side to finish", server_task)
        .await
        .unwrap();
    stop(running).await;
}

#[tokio::test]
async fn a_stalled_stream_does_not_stall_another() {
    let relay = FakeRelay::start(FakeRelayConfig::default()).await;
    let (h, mut outputs) = handler(None);
    let mut running = start(&relay, &h);
    connected(&mut running).await;
    let (server, client) = tls_material();
    let port = relay.front_door_port();

    // Stream A: a browser pushes 1 MiB that nobody reads yet.
    let stalled_total = 1024 * 1024;
    let mut stalled_browser = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut payload = client_hello("bear.relay.test");
    let hello_len = payload.len();
    payload.extend(std::iter::repeat_n(7_u8, stalled_total));
    let writer = tokio::spawn(async move {
        stalled_browser.write_all(&payload).await.ok();
        stalled_browser
    });
    let mut stalled = next_stream(&mut outputs).await;

    // Stream B: a full TLS exchange completes while A is stuck.
    let browser = tokio::spawn(tls_client_exchange(
        port,
        "laptop.bear.relay.test",
        client,
        b"ping".to_vec(),
    ));
    let live = next_stream(&mut outputs).await;
    assert_eq!(live.host, "laptop.bear.relay.test");
    let live_task = tokio::spawn(tls_serve(server, live.io, 4, b"pong".to_vec()));
    let got = wait::guarded("the browser to read the live reply", browser)
        .await
        .unwrap();
    assert_eq!(got, b"pong");
    wait::guarded("the live stream's server side to finish", live_task)
        .await
        .unwrap();

    // A never lost anything: reading it now drains every byte.
    let mut seen = 0;
    let mut buf = vec![0; 64 * 1024];
    while seen < hello_len + stalled_total {
        let n = wait::guarded(
            "the stalled stream's buffered bytes",
            stalled.io.read(&mut buf),
        )
        .await
        .unwrap();
        assert_ne!(n, 0, "stream A ended early");
        seen += n;
    }
    drop(writer.await.unwrap());
    stop(running).await;
}
