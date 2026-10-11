//! A fake relay for tests: a loopback server speaking the tunnel protocol plus a TCP front door that carries raw connections to the
//! connected instance as v2 streams, with the same credit rules as the real
//! relay.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore, mpsc};
use uuid::Uuid;

use super::frames::{V2Frame, WireHosts};
use crate::remote_access::types::{Hostnames, normalize_host};
use crate::util::spawn_in_span;

const WINDOW: usize = 256 * 1024;
const MAX_CHUNK: usize = 64 * 1024;

/// How the fake relay answers `/tunnel/v2/register`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V2Mode {
    /// Speak tunnel v2.
    Serve,
    /// Answer the upgrade with this HTTP status (404 for a relay without v2).
    Respond(u16),
}

/// What the fake relay does.
#[derive(Clone)]
pub(crate) struct FakeRelayConfig {
    /// The bearer token instances must present.
    pub token: String,
    pub user: String,
    pub instance: String,
    /// Host names announced in `connected` (and used by the front door).
    pub hosts: Hostnames,
    pub v2: V2Mode,
    /// Answer every challenge claim with busy.
    pub claim_busy: bool,
    /// What a grant request gets: the grant, or the refusal reason.
    pub grant: Result<String, String>,
    pub keepalive_interval_secs: u64,
    /// The port the front door listens on; a free one when `None`.
    pub door_port: Option<u16>,
}

impl Default for FakeRelayConfig {
    fn default() -> Self {
        Self {
            token: "rst_test".to_string(),
            user: "bear".to_string(),
            instance: "laptop".to_string(),
            hosts: Hostnames::derive("bear", "laptop", "relay.test"),
            v2: V2Mode::Serve,
            claim_busy: false,
            grant: Ok("test.grant.jws".to_string()),
            keepalive_interval_secs: 30,
            door_port: None,
        }
    }
}

/// Browser-side state of one stream the front door carries.
struct DoorStream {
    credit: Semaphore,
    to_browser: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
}

/// One live v2 tunnel connection.
struct Conn {
    out: mpsc::UnboundedSender<Message>,
    streams: Mutex<HashMap<Uuid, Arc<DoorStream>>>,
    close: Notify,
}

struct Shared {
    config: FakeRelayConfig,
    conn: Mutex<Option<Arc<Conn>>>,
    frames: Mutex<Vec<V2Frame>>,
    frames_changed: Notify,
    capabilities: Mutex<Vec<String>>,
    v2_connections: AtomicUsize,
    v2_attempts: AtomicUsize,
    live_v2: AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running fake relay. Dropping it stops the servers.
pub(crate) struct FakeRelay {
    shared: Arc<Shared>,
    addr: SocketAddr,
    door_port: u16,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for FakeRelay {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl FakeRelay {
    /// Start the relay and its front door on free loopback ports.
    pub(crate) async fn start(config: FakeRelayConfig) -> Self {
        let shared = Arc::new(Shared {
            config,
            conn: Mutex::new(None),
            frames: Mutex::new(Vec::new()),
            frames_changed: Notify::new(),
            capabilities: Mutex::new(Vec::new()),
            v2_connections: AtomicUsize::new(0),
            v2_attempts: AtomicUsize::new(0),
            live_v2: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/tunnel/v2/register", get(v2_register))
            .with_state(Arc::clone(&shared));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
        let addr = listener.local_addr().expect("relay addr");
        let server = spawn_in_span(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::debug!(error = %e, "fake relay server ended");
            }
        });
        let door = TcpListener::bind(("127.0.0.1", shared.config.door_port.unwrap_or(0)))
            .await
            .expect("bind door");
        let door_port = door.local_addr().expect("door addr").port();
        let door_task = spawn_in_span(front_door(door, Arc::clone(&shared)));
        Self {
            shared,
            addr,
            door_port,
            tasks: vec![server, door_task],
        }
    }

    /// The registration URL a tunnel client is configured with.
    pub(crate) fn ws_url(&self) -> String {
        format!("ws://{}/tunnel/v2/register", self.addr)
    }

    /// The port the TCP front door listens on.
    pub(crate) fn front_door_port(&self) -> u16 {
        self.door_port
    }

    /// How many v2 tunnels are connected right now.
    pub(crate) fn connected_instances(&self) -> usize {
        self.shared.live_v2.load(Ordering::SeqCst)
    }

    /// How many v2 tunnels have ever connected.
    pub(crate) fn v2_connections(&self) -> usize {
        self.shared.v2_connections.load(Ordering::SeqCst)
    }

    /// How many upgrade attempts reached the v2 endpoint (including refused).
    pub(crate) fn v2_attempts(&self) -> usize {
        self.shared.v2_attempts.load(Ordering::SeqCst)
    }

    /// The capabilities headers v2 upgrades presented.
    pub(crate) fn capabilities_seen(&self) -> Vec<String> {
        lock(&self.shared.capabilities).clone()
    }

    /// Wait for a received frame matching `matches`.
    pub(crate) async fn wait_frame(&self, matches: impl Fn(&V2Frame) -> bool) -> V2Frame {
        crate::testing::wait::guarded(
            "a frame matching the test's expectation at the fake relay",
            async {
                loop {
                    let notified = self.shared.frames_changed.notified();
                    if let Some(frame) = lock(&self.shared.frames).iter().find(|f| matches(f)) {
                        return frame.clone();
                    }
                    notified.await;
                }
            },
        )
        .await
    }

    fn conn(&self) -> Arc<Conn> {
        lock(&self.shared.conn)
            .clone()
            .expect("no instance is connected to the fake relay")
    }

    /// Send a control frame to the connected instance.
    pub(crate) fn send_frame(&self, frame: &V2Frame) {
        let json = serde_json::to_string(frame).expect("serialize frame");
        self.conn().out.send(Message::text(json)).ok();
    }

    /// Send a raw binary message to the connected instance.
    pub(crate) fn send_binary(&self, data: Vec<u8>) {
        self.conn().out.send(Message::Binary(data.into())).ok();
    }

    /// Open a stream without a browser behind it.
    pub(crate) fn open_raw_stream(&self, host: &str, peer_ip: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.send_frame(&V2Frame::StreamOpen {
            stream_id: id,
            host: host.to_string(),
            peer_ip: peer_ip.to_string(),
        });
        id
    }

    /// Send stream data (framed as the protocol requires).
    pub(crate) fn send_stream_data(&self, id: Uuid, payload: &[u8]) {
        let mut message = id.as_bytes().to_vec();
        message.extend_from_slice(payload);
        self.send_binary(message);
    }

    /// Drop the connected instance's tunnel.
    pub(crate) fn drop_connection(&self) {
        self.conn().close.notify_one();
    }
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn authorized(shared: &Shared, headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {}", shared.config.token);
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == expected)
}

async fn v2_register(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    shared.v2_attempts.fetch_add(1, Ordering::SeqCst);
    if let V2Mode::Respond(code) = shared.config.v2 {
        return (status(code), "tunnel v2 unavailable").into_response();
    }
    if !authorized(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let capabilities = headers
        .get("x-residuum-capabilities")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let has_passthrough = capabilities
        .split(',')
        .any(|c| c.trim() == "tls-passthrough");
    lock(&shared.capabilities).push(capabilities);
    if !has_passthrough {
        return (StatusCode::UPGRADE_REQUIRED, "tls-passthrough required").into_response();
    }
    ws.on_upgrade(move |socket| serve_v2(shared, socket))
}

async fn serve_v2(shared: Arc<Shared>, socket: WebSocket) {
    let (mut sink, mut incoming) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let conn = Arc::new(Conn {
        out: out.clone(),
        streams: Mutex::new(HashMap::new()),
        close: Notify::new(),
    });
    let hello = V2Frame::Connected {
        user: shared.config.user.clone(),
        instance: shared.config.instance.clone(),
        keepalive_interval_secs: shared.config.keepalive_interval_secs,
        hosts: WireHosts {
            ui: shared.config.hosts.ui.clone(),
            workbench: shared.config.hosts.workbench.clone(),
            instance: shared.config.hosts.instance.clone(),
        },
        directory_url: "https://relay.test/a2a/bear/agents".to_string(),
        a2a_token: "rsa_fake".to_string(),
    };
    out.send(Message::text(
        serde_json::to_string(&hello).expect("serialize connected"),
    ))
    .ok();
    *lock(&shared.conn) = Some(Arc::clone(&conn));
    shared.v2_connections.fetch_add(1, Ordering::SeqCst);
    shared.live_v2.fetch_add(1, Ordering::SeqCst);

    let writer = spawn_in_span(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });
    loop {
        tokio::select! {
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => handle_text(&shared, &conn, text.as_str()),
                Some(Ok(Message::Binary(data))) => handle_binary(&conn, &data),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            () = conn.close.notified() => break,
        }
    }
    writer.abort();
    lock(&conn.streams).clear();
    {
        let mut current = lock(&shared.conn);
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
            *current = None;
        }
    }
    shared.live_v2.fetch_sub(1, Ordering::SeqCst);
}

fn reply(conn: &Conn, frame: &V2Frame) {
    let json = serde_json::to_string(frame).expect("serialize frame");
    conn.out.send(Message::text(json)).ok();
}

fn handle_text(shared: &Shared, conn: &Arc<Conn>, text: &str) {
    let Ok(frame) = serde_json::from_str::<V2Frame>(text) else {
        return;
    };
    match &frame {
        V2Frame::StreamClose { stream_id, .. } => {
            // Dropping the sender lets the pump deliver what is queued, then
            // end the browser connection.
            if let Some(stream) = lock(&conn.streams).remove(stream_id) {
                lock(&stream.to_browser).take();
            }
        }
        V2Frame::StreamCredit { stream_id, bytes } => {
            if let Some(stream) = lock(&conn.streams).get(stream_id) {
                let room = WINDOW.saturating_sub(stream.credit.available_permits());
                let grant = usize::try_from(*bytes).unwrap_or(usize::MAX).min(room);
                stream.credit.add_permits(grant);
            }
        }
        V2Frame::ChallengeClaim { names } => {
            let answer = if shared.config.claim_busy {
                V2Frame::ChallengeBusy {
                    names: names.clone(),
                }
            } else {
                V2Frame::ChallengeGranted {
                    names: names.clone(),
                }
            };
            reply(conn, &answer);
        }
        V2Frame::PinGrantRequest { purpose } => {
            let answer = match &shared.config.grant {
                Ok(grant) => V2Frame::PinGrant {
                    purpose: purpose.clone(),
                    grant: grant.clone(),
                },
                Err(reason) => V2Frame::PinGrantError {
                    purpose: purpose.clone(),
                    reason: reason.clone(),
                },
            };
            reply(conn, &answer);
        }
        V2Frame::Ping => reply(conn, &V2Frame::Pong),
        V2Frame::Connected { .. }
        | V2Frame::Pong
        | V2Frame::AgentsUpdate { .. }
        | V2Frame::StreamOpen { .. }
        | V2Frame::ChallengeRelease { .. }
        | V2Frame::ChallengeGranted { .. }
        | V2Frame::ChallengeBusy { .. }
        | V2Frame::InstancesUpdate { .. }
        | V2Frame::ActivateInstance { .. }
        | V2Frame::PinGrant { .. }
        | V2Frame::PinGrantError { .. } => {}
    }
    lock(&shared.frames).push(frame);
    shared.frames_changed.notify_waiters();
}

fn handle_binary(conn: &Conn, data: &[u8]) {
    let Some((id, payload)) = data.split_first_chunk::<16>() else {
        return;
    };
    let id = Uuid::from_bytes(*id);
    let stream = lock(&conn.streams).get(&id).cloned();
    if let Some(stream) = stream
        && let Some(tx) = lock(&stream.to_browser).as_ref()
    {
        tx.send(payload.to_vec()).ok();
    }
}

/// Outcome of looking for a server name in buffered `ClientHello` bytes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sni {
    /// More bytes are needed.
    Incomplete,
    /// The hello names this host.
    Found(String),
    /// Not a `ClientHello` with a server name.
    Invalid,
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(n)?;
        self.0 = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<usize> {
        let [b] = *self.take(1)?.first_chunk::<1>()?;
        Some(usize::from(b))
    }

    fn u16(&mut self) -> Option<usize> {
        let bytes = *self.take(2)?.first_chunk::<2>()?;
        Some(usize::from(u16::from_be_bytes(bytes)))
    }
}

/// Find the SNI host in a TLS record holding a `ClientHello`.
pub(crate) fn parse_sni(buf: &[u8]) -> Sni {
    let Some((header, rest)) = buf.split_first_chunk::<5>() else {
        return Sni::Incomplete;
    };
    if header.first() != Some(&0x16) {
        return Sni::Invalid;
    }
    let record_len = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let Some(body) = rest.get(..record_len) else {
        return Sni::Incomplete;
    };
    let parsed = (|| {
        let mut c = Cursor(body);
        if c.u8()? != 0x01 {
            return None;
        }
        c.take(3)?;
        c.take(2 + 32)?;
        let sid = c.u8()?;
        c.take(sid)?;
        let suites = c.u16()?;
        c.take(suites)?;
        let comp = c.u8()?;
        c.take(comp)?;
        let ext_len = c.u16()?;
        let mut ext = Cursor(c.take(ext_len)?);
        while !ext.0.is_empty() {
            let kind = ext.u16()?;
            let len = ext.u16()?;
            let data = ext.take(len)?;
            if kind == 0 {
                let mut e = Cursor(data);
                e.u16()?;
                if e.u8()? != 0 {
                    return None;
                }
                let name_len = e.u16()?;
                let name = e.take(name_len)?;
                return String::from_utf8(name.to_vec()).ok();
            }
        }
        None
    })();
    parsed.map_or(Sni::Invalid, Sni::Found)
}

/// A minimal `ClientHello` record naming `sni`, for tests that need bytes the
/// front door accepts without running a TLS client.
pub(crate) fn client_hello(sni: &str) -> Vec<u8> {
    let name = sni.as_bytes();
    let mut sni_ext = Vec::new();
    sni_ext.extend_from_slice(
        &u16::try_from(name.len() + 3)
            .expect("short name")
            .to_be_bytes(),
    );
    sni_ext.push(0);
    sni_ext.extend_from_slice(&u16::try_from(name.len()).expect("short name").to_be_bytes());
    sni_ext.extend_from_slice(name);
    let mut extensions = vec![0, 0];
    extensions.extend_from_slice(&u16::try_from(sni_ext.len()).expect("short").to_be_bytes());
    extensions.extend_from_slice(&sni_ext);
    let mut hello = vec![3, 3];
    hello.extend_from_slice(&[0; 32]);
    hello.push(0);
    hello.extend_from_slice(&[0, 2, 0x13, 0x01]);
    hello.extend_from_slice(&[1, 0]);
    hello.extend_from_slice(
        &u16::try_from(extensions.len())
            .expect("short")
            .to_be_bytes(),
    );
    hello.extend_from_slice(&extensions);
    let mut handshake = vec![1];
    let len = u32::try_from(hello.len()).expect("short");
    handshake.extend_from_slice(&len.to_be_bytes()[1..]);
    handshake.extend_from_slice(&hello);
    let mut record = vec![0x16, 3, 1];
    record.extend_from_slice(&u16::try_from(handshake.len()).expect("short").to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

async fn front_door(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((socket, peer)) = listener.accept().await else {
            return;
        };
        let shared = Arc::clone(&shared);
        spawn_in_span(async move {
            carry_connection(shared, socket, peer).await;
        });
    }
}

async fn read_hello(socket: &mut TcpStream) -> Option<(Vec<u8>, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match parse_sni(&buf) {
            Sni::Found(host) => return Some((buf, normalize_host(&host))),
            Sni::Invalid => return None,
            Sni::Incomplete => {}
        }
        if buf.len() > 20_000 {
            return None;
        }
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(chunk.get(..n)?);
    }
}

async fn carry_connection(shared: Arc<Shared>, mut socket: TcpStream, peer: SocketAddr) {
    let Some((hello, host)) = read_hello(&mut socket).await else {
        return;
    };
    let Some(conn) = lock(&shared.conn).clone() else {
        return;
    };
    let id = Uuid::new_v4();
    let (to_browser, mut from_instance) = mpsc::unbounded_channel::<Vec<u8>>();
    let stream = Arc::new(DoorStream {
        credit: Semaphore::new(WINDOW),
        to_browser: Mutex::new(Some(to_browser)),
    });
    lock(&conn.streams).insert(id, Arc::clone(&stream));
    reply(
        &conn,
        &V2Frame::StreamOpen {
            stream_id: id,
            host,
            peer_ip: peer.ip().to_string(),
        },
    );
    let (mut browser_rx, mut browser_tx) = socket.into_split();

    let upload = async {
        let mut pending = hello;
        let mut buffer = vec![0_u8; MAX_CHUNK];
        loop {
            if pending.is_empty() {
                let n = browser_rx.read(&mut buffer).await.ok()?;
                if n == 0 {
                    return Some(());
                }
                pending = buffer.get(..n)?.to_vec();
            }
            let n = pending.len().min(MAX_CHUNK);
            stream
                .credit
                .acquire_many(u32::try_from(n).ok()?)
                .await
                .ok()?
                .forget();
            let rest = pending.split_off(n);
            let mut message = id.as_bytes().to_vec();
            message.extend_from_slice(&pending);
            conn.out.send(Message::Binary(message.into())).ok()?;
            pending = rest;
        }
    };
    let download = async {
        while let Some(data) = from_instance.recv().await {
            if browser_tx.write_all(&data).await.is_err() {
                return false;
            }
            reply(
                &conn,
                &V2Frame::StreamCredit {
                    stream_id: id,
                    bytes: data.len() as u64,
                },
            );
        }
        browser_tx.shutdown().await.ok();
        true
    };
    tokio::select! {
        _ = upload => {
            reply(&conn, &V2Frame::StreamClose { stream_id: id, reason: None });
        }
        _ = download => {}
    }
    lock(&conn.streams).remove(&id);
}
