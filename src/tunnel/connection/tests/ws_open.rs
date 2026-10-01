//! Socket opens from the relay: which listener each surface reaches, what the
//! loopback hop carries, and what a listener that isn't running answers.
//!
//! The local listeners are real: the artifacts listener is the workbench
//! router with a hub stand-in bound behind it, so a workbench-surface socket
//! travels the same tunnel, artifacts listener and hub router hops it does in
//! production.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Extension;
use axum::extract::ws::{Message as SocketMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{MethodRouter, get};

use super::*;
use crate::gateway::remote_control_guard::reject_remote_shutdown_and_disconnect;
use crate::tunnel::{TUNNEL_NONCE_HEADER, tunnel_nonce};
use crate::workbench::forward::{ArtifactsOrigin, HubApi};

/// An API socket on both stand-ins.
const API_SOCKET: &str = "/api/hub/ws";

/// A route the remote-control guard is mounted on, as a socket. The real
/// guarded routes are `POST`s; this one carries the real guard layer so the
/// test sees whether the nonce reaches it across each hop.
const GUARDED_SOCKET: &str = "/api/hub/cloud/disconnect";

/// A stand-in socket route. It greets with its label, the tunnel nonce its
/// upgrade request carried and whether the request came through the artifacts
/// origin, then echoes every message with its label.
fn socket_route(label: &'static str) -> MethodRouter {
    get(
        move |ws: WebSocketUpgrade,
              headers: HeaderMap,
              origin: Option<Extension<ArtifactsOrigin>>| async move {
            let nonce = headers
                .get(TUNNEL_NONCE_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("absent")
                .to_string();
            let through_artifacts_origin = origin.is_some();
            ws.on_upgrade(move |mut socket: WebSocket| async move {
                let greeting =
                    format!("{label} nonce={nonce} artifacts_origin={through_artifacts_origin}");
                if socket
                    .send(SocketMessage::Text(greeting.into()))
                    .await
                    .is_err()
                {
                    return;
                }
                while let Some(Ok(SocketMessage::Text(text))) = socket.recv().await {
                    let echo = format!("{label} echo {text}");
                    if socket.send(SocketMessage::Text(echo.into())).await.is_err() {
                        break;
                    }
                }
            })
        },
    )
}

/// Both stand-in routes under the real remote-control guard layer.
fn standin_routes(label: &'static str) -> axum::Router {
    let guarded = axum::Router::new()
        .route(GUARDED_SOCKET, socket_route(label))
        .route_layer(axum::middleware::from_fn(
            reject_remote_shutdown_and_disconnect,
        ));
    axum::Router::new()
        .route(API_SOCKET, socket_route(label))
        .merge(guarded)
}

async fn count_connection(
    State(connections): State<Arc<AtomicUsize>>,
    req: Request,
    next: Next,
) -> Response {
    connections.fetch_add(1, Ordering::SeqCst);
    next.run(req).await
}

/// A stand-in listener that counts every request it receives.
fn counting_listener(label: &'static str, connections: &Arc<AtomicUsize>) -> axum::Router {
    standin_routes(label).layer(axum::middleware::from_fn_with_state(
        Arc::clone(connections),
        count_connection,
    ))
}

/// Serves `app` on a free loopback port.
async fn serve(app: axum::Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    crate::util::spawn_in_span(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A main listener, optionally an artifacts listener in front of a hub
/// stand-in, and a tunnel whose relay end the test reads.
struct WsHarness {
    write: Arc<Mutex<TunnelSink>>,
    relay: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    targets: ForwardTargets,
    ws_events_tx: mpsc::Sender<WsChannelEvent>,
    ws_events_rx: mpsc::Receiver<WsChannelEvent>,
    main_connections: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn harness(artifacts_listener_running: bool) -> WsHarness {
    let dir = tempfile::tempdir().unwrap();
    let main_connections = Arc::new(AtomicUsize::new(0));
    let main = serve(counting_listener("main", &main_connections)).await;
    let workbench = if artifacts_listener_running {
        let api = HubApi::new();
        api.bind(standin_routes("hub"));
        Some(
            serve(crate::workbench::server::router(
                dir.path().to_path_buf(),
                api,
            ))
            .await,
        )
    } else {
        None
    };
    let (write, relay) = loopback_ws().await;
    let (ws_events_tx, ws_events_rx) = mpsc::channel(16);
    WsHarness {
        write,
        relay,
        targets: ForwardTargets {
            main,
            workbench,
            a2a: None,
        },
        ws_events_tx,
        ws_events_rx,
        main_connections,
        _dir: dir,
    }
}

/// The headers of a socket open whose client supplied `value` for the tunnel
/// header, or none at all.
fn nonce_header(value: Option<&str>) -> HashMap<String, String> {
    value
        .into_iter()
        .map(|v| ("X-Residuum-Tunnel".to_string(), v.to_string()))
        .collect()
}

impl WsHarness {
    /// Delivers `frame` to the tunnel as the relay would. A successful open
    /// yields the sender for messages from the tunnel to the local socket; a
    /// failed one yields the reason it carried.
    async fn send_open(&mut self, frame: TunnelFrame) -> Result<mpsc::Sender<String>, String> {
        let client = forward_http::forwarding_client().unwrap();
        let a2a_client = forward_a2a::forwarding_client().unwrap();
        let clients = TunnelClients {
            client: &client,
            a2a_client: &a2a_client,
        };
        let mut local_ws_channels = HashMap::new();
        let mut streams = HashMap::new();
        let (done_tx, _done_rx) = mpsc::channel(1);
        let mut tracker = A2aStreamTracker {
            streams: &mut streams,
            done_tx: &done_tx,
        };
        with_timeout(handle_frame(
            frame,
            &clients,
            self.targets,
            &self.write,
            &mut local_ws_channels,
            &self.ws_events_tx,
            &mut tracker,
        ))
        .await;

        let answer = recv_ws_frame(&mut self.relay).await;
        let TunnelFrame::WsOpenResult {
            success, reason, ..
        } = answer
        else {
            panic!("expected a WsOpenResult, got {answer:?}");
        };
        if success {
            assert_eq!(reason, None, "a successful open has nothing to explain");
            // An earlier open's channel may end first: its sender was dropped.
            let sender = loop {
                let event = with_timeout(self.ws_events_rx.recv())
                    .await
                    .expect("a successful open registers its channel");
                if let WsChannelEvent::Opened { sender, .. } = event {
                    break sender;
                }
            };
            Ok(sender)
        } else {
            Err(reason.expect("a failed open must say why"))
        }
    }

    async fn open(
        &mut self,
        surface: Option<Surface>,
        path: &str,
        headers: HashMap<String, String>,
    ) -> Result<mpsc::Sender<String>, String> {
        self.send_open(TunnelFrame::WsOpen {
            channel_id: "ch-1".to_string(),
            path: path.to_string(),
            headers,
            surface,
        })
        .await
    }

    /// The text of the next socket message a local listener sent through the
    /// tunnel.
    async fn next_message(&mut self) -> String {
        let frame = recv_ws_frame(&mut self.relay).await;
        let TunnelFrame::WsMessage { data, .. } = frame else {
            panic!("expected a socket message, got {frame:?}");
        };
        data
    }
}

#[test]
fn a_socket_open_picks_its_listener_by_surface_and_never_falls_back() {
    let running = ForwardTargets {
        main: 7700,
        workbench: Some(7702),
        a2a: Some(7703),
    };
    assert_eq!(ws_open_port(running, None), Ok(7700));
    assert_eq!(ws_open_port(running, Some(Surface::Workbench)), Ok(7702));
    assert!(
        ws_open_port(running, Some(Surface::A2a)).is_err(),
        "the A2A listener serves no sockets"
    );

    let down = ForwardTargets {
        main: 7700,
        workbench: None,
        a2a: None,
    };
    assert!(
        ws_open_port(down, Some(Surface::Workbench)).is_err(),
        "a workbench socket must never fall back to the main listener"
    );
}

#[tokio::test]
async fn a_socket_open_without_a_surface_reaches_the_main_listener() {
    let mut h = harness(true).await;

    let sender = h.open(None, API_SOCKET, HashMap::new()).await.unwrap();
    let greeting = h.next_message().await;
    assert!(greeting.starts_with("main "), "{greeting}");
    assert!(
        greeting.ends_with("artifacts_origin=false"),
        "the main listener is not the artifacts origin: {greeting}"
    );

    sender.send("ping".to_string()).await.unwrap();
    assert_eq!(h.next_message().await, "main echo ping");
}

#[tokio::test]
async fn a_frame_from_a_relay_that_sends_no_surface_reaches_the_main_listener() {
    let mut h = harness(true).await;
    let frame: TunnelFrame = serde_json::from_str(&format!(
        r#"{{"type":"ws_open","channel_id":"ch-old","path":"{API_SOCKET}","headers":{{}}}}"#
    ))
    .unwrap();

    h.send_open(frame).await.unwrap();

    let greeting = h.next_message().await;
    assert!(greeting.starts_with("main "), "{greeting}");
}

#[tokio::test]
async fn a_workbench_socket_open_reaches_the_hub_through_the_artifacts_listener() {
    let mut h = harness(true).await;

    let sender = h
        .open(Some(Surface::Workbench), API_SOCKET, HashMap::new())
        .await
        .unwrap();
    let greeting = h.next_message().await;
    assert!(greeting.starts_with("hub "), "{greeting}");
    assert!(
        greeting.ends_with("artifacts_origin=true"),
        "the hub router must see the socket as arriving through the artifacts origin: {greeting}"
    );

    sender.send("ping".to_string()).await.unwrap();
    assert_eq!(h.next_message().await, "hub echo ping");
    assert_eq!(
        h.main_connections.load(Ordering::SeqCst),
        0,
        "the main listener must not see a workbench socket"
    );
}

#[tokio::test]
async fn a_workbench_socket_open_with_no_artifacts_listener_fails_with_a_reason() {
    let mut h = harness(false).await;

    let reason = h
        .open(Some(Surface::Workbench), API_SOCKET, HashMap::new())
        .await
        .unwrap_err();

    assert!(
        reason.contains("artifacts listener isn't running"),
        "the reason must say what is missing: {reason}"
    );
    assert_eq!(
        h.main_connections.load(Ordering::SeqCst),
        0,
        "a workbench socket must never fall back to the main listener"
    );
    assert!(
        h.ws_events_rx.try_recv().is_err(),
        "a refused open registers no channel"
    );
}

#[tokio::test]
async fn an_a2a_socket_open_is_refused_and_reaches_no_listener() {
    let mut h = harness(true).await;
    let a2a_connections = Arc::new(AtomicUsize::new(0));
    h.targets.a2a = Some(serve(counting_listener("a2a", &a2a_connections)).await);

    let reason = h
        .open(Some(Surface::A2a), API_SOCKET, HashMap::new())
        .await
        .unwrap_err();

    assert!(reason.contains("A2A"), "{reason}");
    assert_eq!(a2a_connections.load(Ordering::SeqCst), 0);
    assert_eq!(h.main_connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_loopback_hop_carries_this_processs_nonce_never_the_clients() {
    let mut h = harness(true).await;
    let expected = format!("nonce={}", tunnel_nonce());

    for surface in [None, Some(Surface::Workbench)] {
        for supplied in [None, Some("spoofed-value")] {
            h.open(surface, API_SOCKET, nonce_header(supplied))
                .await
                .unwrap();
            let greeting = h.next_message().await;
            assert!(
                greeting.contains(&expected),
                "{surface:?} with the client's header {supplied:?}: {greeting}"
            );
        }
    }
}

#[tokio::test]
async fn a_socket_open_to_the_remote_control_guard_is_refused_on_both_surfaces() {
    let mut h = harness(true).await;

    for surface in [None, Some(Surface::Workbench)] {
        // A client that forges the tunnel header, or leaves it out, is marked
        // as tunnel-forwarded all the same.
        for supplied in [None, Some("spoofed-value")] {
            let reason = h
                .open(surface, GUARDED_SOCKET, nonce_header(supplied))
                .await
                .unwrap_err();
            assert!(
                reason.contains("403"),
                "{surface:?} with the client's header {supplied:?}: {reason}"
            );
        }
    }

    // The same routes answer a client that isn't on the tunnel.
    for port in [Some(h.targets.main), h.targets.workbench] {
        let port = port.expect("both listeners run");
        let direct =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}{GUARDED_SOCKET}"))
                .await;
        assert!(
            direct.is_ok(),
            "a local client reaches the guarded route on port {port}: {:?}",
            direct.err()
        );
    }
}
