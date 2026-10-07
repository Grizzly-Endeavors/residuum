//! In-process tests: a TLS client over a duplex pipe into `Engine::serve`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, Request, StatusCode, Version, header};
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use super::engine::{Engine, EngineDeps, EngineRouters};
use super::tls::{CertBundle, CertResolver};
use super::types::Hostnames;
use crate::pairing::remote::RemoteTransport;

const PEER: &str = "203.0.113.7";

type Client = TlsStream<tokio::io::DuplexStream>;

struct Harness {
    engine: Arc<Engine>,
    names: Hostnames,
    client_config: Arc<ClientConfig>,
    // Held so the watch channels stay open.
    _hostnames_tx: watch::Sender<Option<Hostnames>>,
    _teams_tx: watch::Sender<BTreeMap<String, u16>>,
}

fn ui_router() -> Router {
    async fn whoami(
        Extension(transport): Extension<RemoteTransport>,
        headers: HeaderMap,
    ) -> String {
        format!(
            "ui peer={} origin={} host={}",
            transport.peer_ip.unwrap_or_default(),
            transport.origin.unwrap_or_default(),
            headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
        )
    }
    async fn tunnel_header(headers: HeaderMap) -> String {
        let seen = [
            "x-residuum-tunnel",
            "x-residuum-sibling",
            "x-residuum-a2a-caller",
            "x-real-ip",
        ]
        .map(|name| format!("{name}={}", headers.contains_key(name)));
        seen.join(" ")
    }
    async fn ws(upgrade: WebSocketUpgrade) -> impl IntoResponse {
        upgrade.on_upgrade(|mut socket: WebSocket| async move {
            while let Some(Ok(Message::Text(text))) = socket.recv().await {
                if socket
                    .send(Message::Text(format!("echo:{text}").into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
    }
    Router::new()
        .route("/whoami", get(whoami))
        .route("/headers", get(tunnel_header))
        .route("/ws", get(ws))
}

fn workbench_router() -> Router {
    Router::new().route("/whoami", get(|| async { "workbench" }))
}

fn harness(a2a_port: Option<u16>, teams: BTreeMap<String, u16>, with_identity: bool) -> Harness {
    let names = Hostnames::derive("bear", "laptop", "agent-residuum.com");
    let all: Vec<String> = names.all().iter().map(|n| (*n).to_string()).collect();
    let certified = rcgen::generate_simple_self_signed(all.clone()).expect("self-signed cert");
    let cert_der = certified.cert.der().clone();
    let now = Utc::now();
    let bundle = CertBundle {
        chain_pem: certified.cert.pem(),
        key_pem: certified.signing_key.serialize_pem(),
        not_before: now - ChronoDuration::hours(1),
        not_after: now + ChronoDuration::days(60),
        names: all,
    };
    let resolver = CertResolver::new();
    resolver.set_certificate(&bundle).expect("set certificate");

    let (hostnames_tx, hostnames_rx) = watch::channel(with_identity.then(|| names.clone()));
    let (teams_tx, teams_rx) = watch::channel(teams);
    let engine = Engine::new(EngineDeps {
        resolver,
        hostnames: hostnames_rx,
        routers: EngineRouters {
            ui: ui_router(),
            workbench: workbench_router(),
        },
        sibling_routes: Router::new().route(
            "/_sibling/join/nonce",
            get(|req: axum::extract::Request| async move {
                let peer = req
                    .extensions()
                    .get::<crate::pairing::remote::RemoteTransport>()
                    .and_then(|t| t.peer_ip.clone())
                    .unwrap_or_default();
                format!("sibling-routes peer={peer}")
            }),
        ),
        a2a_port,
        teams_ports: teams_rx,
    });

    let mut roots = RootCertStore::empty();
    roots.add(cert_der).expect("trust the test certificate");
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let client_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();

    Harness {
        engine,
        names,
        client_config: Arc::new(client_config),
        _hostnames_tx: hostnames_tx,
        _teams_tx: teams_tx,
    }
}

impl Harness {
    /// Open a TLS connection to the engine presenting `sni` and `alpn`; the
    /// relay host the engine is told is always the UI name.
    async fn connect(&self, sni: &str, alpn: &[&[u8]]) -> std::io::Result<Client> {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let engine = Arc::clone(&self.engine);
        let relay_host = self.names.ui.clone();
        tokio::spawn(async move {
            engine.serve(relay_host, PEER.to_string(), server_io).await;
        });
        let mut config = (*self.client_config).clone();
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let server_name = ServerName::try_from(sni.to_string()).expect("server name");
        TlsConnector::from(Arc::new(config))
            .connect(server_name, client_io)
            .await
    }
}

struct Reply {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    body: String,
}

/// One request over a fresh connection. HTTP/2 when the connection negotiated it.
async fn request(
    client: Client,
    method: Method,
    host: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let h2 = client.get_ref().1.alpn_protocol() == Some(b"h2");
    let mut builder = Request::builder().method(method);
    builder = if h2 {
        builder.uri(format!("https://{host}{path}"))
    } else {
        builder.uri(path).header(header::HOST, host)
    };
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let req = builder
        .body(Empty::<axum::body::Bytes>::new())
        .expect("request");
    let io = TokioIo::new(client);
    let response = if h2 {
        let (mut sender, conn) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
            .await
            .expect("h2 handshake");
        tokio::spawn(conn);
        sender.send_request(req).await.expect("h2 request")
    } else {
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .expect("h1 handshake");
        tokio::spawn(conn);
        sender.send_request(req).await.expect("h1 request")
    };
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.expect("body").to_bytes();
    Reply {
        status: parts.status,
        version: parts.version,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get_h1(h: &Harness, host: &str, path: &str) -> Reply {
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    request(client, Method::GET, host, path, &[]).await
}

#[tokio::test]
async fn ui_host_reaches_ui_router_with_remote_transport() {
    let h = harness(None, BTreeMap::new(), true);
    let reply = get_h1(&h, &h.names.ui.clone(), "/whoami").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body,
        format!(
            "ui peer={PEER} origin=https://{ui} host={ui}",
            ui = h.names.ui
        )
    );
}

#[tokio::test]
async fn workbench_host_reaches_workbench_router() {
    let h = harness(None, BTreeMap::new(), true);
    let reply = get_h1(&h, &h.names.workbench.clone(), "/whoami").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, "workbench");
}

#[tokio::test]
async fn host_with_port_and_case_still_routes() {
    let h = harness(None, BTreeMap::new(), true);
    let host = format!("{}:443", h.names.workbench.to_uppercase());
    let reply = get_h1(&h, &host, "/whoami").await;
    assert_eq!(reply.body, "workbench");
}

#[tokio::test]
async fn unknown_host_over_valid_sni_gets_421() {
    let h = harness(None, BTreeMap::new(), true);
    let reply = get_h1(&h, "evil.example.com", "/whoami").await;
    assert_eq!(reply.status, StatusCode::MISDIRECTED_REQUEST);
    assert!(reply.body.contains("isn't served"));
}

#[tokio::test]
async fn coalesced_connection_routes_by_host_not_sni() {
    let h = harness(None, BTreeMap::new(), true);
    // SNI is the UI name; the request is for the workbench name.
    let reply = get_h1(&h, &h.names.workbench.clone(), "/whoami").await;
    assert_eq!(reply.body, "workbench");
}

#[tokio::test]
async fn sni_other_than_relay_host_is_closed() {
    let h = harness(None, BTreeMap::new(), true);
    // A valid name of this instance, but not the one the relay opened the stream for.
    let result = h.connect(&h.names.workbench.clone(), &[b"http/1.1"]).await;
    assert!(result.is_err(), "handshake must not complete");
}

#[tokio::test]
async fn sni_outside_hostnames_is_closed() {
    let h = harness(None, BTreeMap::new(), true);
    let result = h.connect("other.example.com", &[b"http/1.1"]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn no_identity_closes_every_connection() {
    let h = harness(None, BTreeMap::new(), false);
    let result = h.connect(&h.names.ui.clone(), &[b"http/1.1"]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn client_hello_without_sni_is_closed() {
    let h = harness(None, BTreeMap::new(), true);
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let engine = Arc::clone(&h.engine);
    let relay_host = h.names.ui.clone();
    tokio::spawn(async move { engine.serve(relay_host, PEER.to_string(), server_io).await });
    let mut config = (*h.client_config).clone();
    config.enable_sni = false;
    let ip = ServerName::try_from("192.0.2.1").expect("ip name");
    let result = TlsConnector::from(Arc::new(config))
        .connect(ip, client_io)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn forged_internal_headers_are_stripped() {
    let h = harness(None, BTreeMap::new(), true);
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let reply = request(
        client,
        Method::GET,
        &h.names.ui.clone(),
        "/headers",
        &[
            ("x-residuum-tunnel", "forged"),
            ("x-residuum-sibling", "evil"),
            ("x-residuum-a2a-caller", "key:root"),
            ("x-real-ip", "10.0.0.1"),
        ],
    )
    .await;
    assert_eq!(
        reply.body,
        "x-residuum-tunnel=false x-residuum-sibling=false x-residuum-a2a-caller=false x-real-ip=false"
    );
}

#[tokio::test]
async fn http2_and_http1_both_work() {
    let h = harness(None, BTreeMap::new(), true);
    let host = h.names.ui.clone();

    let client = h
        .connect(&host, &[b"h2", b"http/1.1"])
        .await
        .expect("connect h2");
    assert_eq!(client.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
    let reply = request(client, Method::GET, &host, "/whoami", &[]).await;
    assert_eq!(reply.version, Version::HTTP_2);
    assert!(reply.body.starts_with("ui peer="));

    let client_h1 = h.connect(&host, &[b"http/1.1"]).await.expect("connect h1");
    let reply_h1 = request(client_h1, Method::GET, &host, "/whoami", &[]).await;
    assert_eq!(reply_h1.version, Version::HTTP_11);
    assert!(reply_h1.body.starts_with("ui peer="));
}

#[tokio::test]
async fn acme_alpn_connection_ends_after_handshake() {
    let h = harness(None, BTreeMap::new(), true);
    // The test certificate isn't an RFC 8737 challenge certificate, but the
    // engine's behavior depends only on the negotiated ALPN protocol.
    let result = h.connect(&h.names.ui.clone(), &[b"acme-tls/1"]).await;
    // Whether the resolver offers acme-tls/1 depends on a challenge being set;
    // with none set the server falls back to a protocol the client didn't
    // offer, so the handshake may be refused. Both outcomes end without HTTP.
    if let Ok(mut client) = result {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut buf)).await;
        assert!(read.is_ok(), "connection must close");
        assert!(buf.is_empty());
    }
}

#[tokio::test]
async fn websocket_upgrade_reaches_ui_router() {
    let h = harness(None, BTreeMap::new(), true);
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let url = format!("wss://{}/ws", h.names.ui);
    let (mut socket, response) = tokio_tungstenite::client_async(url, client)
        .await
        .expect("websocket handshake");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket
        .send(tokio_tungstenite::tungstenite::Message::text("hi"))
        .await
        .expect("send");
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("timely reply")
        .expect("a frame")
        .expect("ok frame");
    assert_eq!(reply.into_text().expect("text").as_str(), "echo:hi");
}

/// A fake A2A/Teams listener on loopback.
async fn spawn_upstream(release_second_chunk: Arc<Notify>) -> u16 {
    async fn echo(method: Method, uri: axum::http::Uri, headers: HeaderMap) -> impl IntoResponse {
        let body = format!(
            "{method} {uri} host_is_proxy_host={} tunnel={} auth={}",
            headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("agent-residuum.com")),
            headers.contains_key("x-residuum-tunnel"),
            headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("none"),
        );
        ([(header::SET_COOKIE, "session=abc")], body)
    }
    let events = move || {
        let release = Arc::clone(&release_second_chunk);
        async move {
            let stream = futures_util::stream::unfold(0_u8, move |step| {
                let release = Arc::clone(&release);
                async move {
                    match step {
                        0 => Some((
                            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(
                                "data: one\n\n",
                            )),
                            1,
                        )),
                        1 => {
                            release.notified().await;
                            Some((Ok(axum::body::Bytes::from("data: two\n\n")), 2))
                        }
                        _ => None,
                    }
                }
            });
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(stream),
            )
        }
    };
    let router = Router::new()
        .route("/agents/scout/events", get(events))
        .route("/agents/scout/{*rest}", any(echo))
        .route("/agents/scout", any(echo))
        .route(
            crate::interfaces::teams::MESSAGES_PATH,
            post(|body: String| async move { format!("teams got {} bytes", body.len()) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("upstream serves");
    });
    port
}

#[tokio::test]
async fn a2a_proxy_streams_sse_incrementally() {
    let release = Arc::new(Notify::new());
    let port = spawn_upstream(Arc::clone(&release)).await;
    let h = harness(Some(port), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");

    let io = TokioIo::new(client);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .expect("handshake");
    tokio::spawn(conn);
    let req = Request::builder()
        .uri("/a2a/scout/events")
        .header(header::HOST, &host)
        .body(Empty::<axum::body::Bytes>::new())
        .expect("request");
    let response = sender.send_request(req).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    let mut body = response.into_body();

    let first = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("first chunk arrives before the stream ends")
        .expect("frame")
        .expect("ok frame");
    assert_eq!(first.into_data().expect("data").as_ref(), b"data: one\n\n");

    release.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("second chunk")
        .expect("frame")
        .expect("ok frame");
    assert_eq!(second.into_data().expect("data").as_ref(), b"data: two\n\n");
}

#[tokio::test]
async fn a2a_proxy_rewrites_path_keeps_query_and_hardens() {
    let port = spawn_upstream(Arc::new(Notify::new())).await;
    let h = harness(Some(port), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let reply = request(
        client,
        Method::GET,
        &host,
        "/a2a/scout/rest/message:send?x=1&y=2",
        &[
            ("x-residuum-tunnel", "forged"),
            ("authorization", "Bearer t"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(
        reply.body,
        "GET /agents/scout/rest/message:send?x=1&y=2 host_is_proxy_host=false tunnel=false auth=Bearer t"
    );
}

#[tokio::test]
async fn a2a_path_traversal_is_refused() {
    let port = spawn_upstream(Arc::new(Notify::new())).await;
    let h = harness(Some(port), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    for bad in [
        "/a2a/scout/%2e%2e/vault",
        "/a2a/scout/%2E%2e/vault",
        "/a2a/scout/../vault",
        "/a2a/x/%2e%2e/y",
        "/a2a/../teams",
        "/a2a/sc%6fut/rest",
    ] {
        let client = h
            .connect(&h.names.ui, &[b"http/1.1"])
            .await
            .expect("connect");
        let reply = request(client, Method::GET, &host, bad, &[]).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{bad}");
        assert!(
            !reply.body.contains("GET /agents"),
            "{bad} reached the listener"
        );
    }
}

#[tokio::test]
async fn a2a_without_listener_says_so() {
    let h = harness(None, BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let reply = request(client, Method::GET, &host, "/a2a/scout/x", &[]).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert!(reply.body.contains("A2A listener isn't running"));
}

#[tokio::test]
async fn a2a_without_agent_is_404() {
    let h = harness(Some(1), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    for path in ["/a2a", "/a2a/"] {
        let client = h
            .connect(&h.names.ui, &[b"http/1.1"])
            .await
            .expect("connect");
        let reply = request(client, Method::GET, &host, path, &[]).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert!(reply.body.contains("didn't name an agent"));
    }
}

#[tokio::test]
async fn a2a_listener_down_is_bad_gateway() {
    // Port 1 on loopback has no listener.
    let h = harness(Some(1), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let reply = request(client, Method::GET, &host, "/a2a/scout/x", &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn instance_host_serves_nothing_else() {
    let h = harness(Some(1), BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let client = h
        .connect(&h.names.ui, &[b"http/1.1"])
        .await
        .expect("connect");
    let reply = request(client, Method::GET, &host, "/whoami", &[]).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a2a_is_rate_limited_per_peer() {
    let h = harness(None, BTreeMap::new(), true);
    let host = h.names.instance.clone();
    let mut limited = None;
    for _ in 0..70 {
        let client = h
            .connect(&h.names.ui, &[b"http/1.1"])
            .await
            .expect("connect");
        let reply = request(client, Method::GET, &host, "/a2a/scout/x", &[]).await;
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(reply);
            break;
        }
    }
    let limited = limited.expect("burst of 60 is exhausted within 70 requests");
    assert!(limited.headers.contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn teams_is_proxied_and_validated() {
    let port = spawn_upstream(Arc::new(Notify::new())).await;
    let teams = BTreeMap::from([("scout".to_string(), port)]);
    let h = harness(None, teams, true);
    let host = h.names.instance.clone();

    let post_to = |path: &'static str, method: Method| {
        let h = &h;
        let host = host.clone();
        async move {
            let client = h
                .connect(&h.names.ui, &[b"http/1.1"])
                .await
                .expect("connect");
            request(client, method, &host, path, &[]).await
        }
    };

    let reply = post_to("/teams/scout", Method::POST).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, "teams got 0 bytes");

    let not_post = post_to("/teams/scout", Method::GET).await;
    assert_eq!(not_post.status, StatusCode::METHOD_NOT_ALLOWED);

    let no_listener = post_to("/teams/nobody", Method::POST).await;
    assert_eq!(no_listener.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        no_listener
            .body
            .contains("isn't set up to receive Teams messages")
    );

    let extra = post_to("/teams/scout/extra", Method::POST).await;
    assert_eq!(extra.status, StatusCode::NOT_FOUND);

    let bad = post_to("/teams/Bad_Name", Method::POST).await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sibling_join_paths_are_served_in_process_on_the_instance_host_only() {
    let h = harness(Some(1), BTreeMap::new(), true);
    let instance = h.names.instance.clone();
    let reply = get_h1(&h, &instance, "/_sibling/join/nonce").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(
        reply.body.starts_with("sibling-routes peer="),
        "{}",
        reply.body
    );
    assert!(
        !reply.body.ends_with("peer="),
        "the join endpoints get the peer address the relay reported: {}",
        reply.body
    );

    // The UI host has no such route, so the device gate's surface is unchanged.
    let ui = h.names.ui.clone();
    let on_ui = get_h1(&h, &ui, "/_sibling/join/nonce").await;
    assert_eq!(on_ui.status, StatusCode::NOT_FOUND);
}
