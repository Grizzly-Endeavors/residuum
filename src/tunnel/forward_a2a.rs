//! Streaming HTTP forwarding for the A2A surface.
//!
//! Unlike the buffered path in [`super::forward_http`], an A2A request's
//! response is sent to the relay as it arrives: `HttpResponseStart`, then zero
//! or more `HttpResponseChunk`s, then `HttpResponseEnd`. This lets a
//! long-lived A2A task (an SSE stream that stays open while the agent works)
//! flow through the tunnel without a body timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::protocol::TunnelFrame;
use super::{ForwardRequest, TUNNEL_NONCE_HEADER, TunnelSink, send_frame, tunnel_nonce};

/// How long to wait for response headers from the local A2A listener. Once
/// headers arrive there is no further timeout: the body may stream for as
/// long as the A2A task runs.
const HEADERS_TIMEOUT: Duration = Duration::from_secs(25);

/// Maximum size, in bytes, of a single `HttpResponseChunk`'s decoded payload.
const MAX_CHUNK_SIZE: usize = 64 * 1024;

/// Build the HTTP client used for A2A streaming forwards.
///
/// Unlike [`super::forward_http::forwarding_client`], this client sets no
/// overall request timeout: an A2A response body can stay open indefinitely
/// (an SSE stream while the agent works). [`HEADERS_TIMEOUT`] bounds only the
/// wait for the response to start.
///
/// # Errors
/// Returns an error if the client cannot be built.
pub(super) fn forwarding_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Forward an A2A-surface HTTP request to the local A2A listener and stream
/// the response back through the tunnel as `HttpResponseStart`,
/// `HttpResponseChunk`(s), and `HttpResponseEnd`.
///
/// Strips any incoming `x-residuum-tunnel` header before adding the tunnel's
/// own — a caller can't forge the header the local A2A listener uses to trust
/// a sibling attestation.
#[tracing::instrument(skip_all, fields(request_id = %request.request_id, method = %request.method, path = %request.path))]
pub(super) async fn stream_forward(
    client: &reqwest::Client,
    port: u16,
    request: ForwardRequest,
    write: &Arc<Mutex<TunnelSink>>,
) {
    let request_id = request.request_id.clone();
    let request_id = request_id.as_str();

    let req = match build_request(client, port, request) {
        Ok(req) => req,
        Err(message) => {
            warn!(request_id, message = %message, "failed to build A2A forward request");
            stream_error(write, request_id, 502, &message).await;
            return;
        }
    };

    let response = match tokio::time::timeout(HEADERS_TIMEOUT, req.send()).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            warn!(request_id, error = %e, "failed to forward A2A request to local listener");
            stream_error(write, request_id, 502, &format!("upstream error: {e}")).await;
            return;
        }
        Err(_) => {
            warn!(
                request_id,
                timeout_secs = HEADERS_TIMEOUT.as_secs(),
                "timed out waiting for A2A response headers from local listener"
            );
            stream_error(
                write,
                request_id,
                502,
                "timed out waiting for a response from the local A2A listener",
            )
            .await;
            return;
        }
    };

    let status = response.status().as_u16();
    let response_headers = collect_response_headers(&response);

    if let Err(e) = send_frame(
        write,
        &TunnelFrame::HttpResponseStart {
            request_id: request_id.to_string(),
            status,
            headers: response_headers,
        },
    )
    .await
    {
        warn!(request_id, error = %e, "failed to send HttpResponseStart");
        return;
    }

    let stream_error_message = stream_body(request_id, response, write).await;

    if let Err(e) = send_frame(
        write,
        &TunnelFrame::HttpResponseEnd {
            request_id: request_id.to_string(),
            error: stream_error_message,
        },
    )
    .await
    {
        warn!(request_id, error = %e, "failed to send HttpResponseEnd");
    }
}

/// Build the outbound request for the local A2A listener: parse the method,
/// attach the forwarded headers (stripping hop-by-hop and any incoming
/// `x-residuum-tunnel`), add the tunnel's own nonce, and decode the body.
///
/// Returns a plain message (always answered as a streamed 502) on the two
/// ways this can fail: an unparseable method or invalid base64 body.
fn build_request(
    client: &reqwest::Client,
    port: u16,
    request: ForwardRequest,
) -> Result<reqwest::RequestBuilder, String> {
    let ForwardRequest {
        method,
        path,
        headers,
        body,
        ..
    } = request;
    let url = format!("http://localhost:{port}{path}");
    debug!(url, "forwarding A2A request to local listener");

    let http_method = method
        .parse::<reqwest::Method>()
        .map_err(|e| format!("unsupported method: {e}"))?;

    let decoded_body = match body {
        Some(ref b64) => Some(
            STANDARD
                .decode(b64)
                .map_err(|e| format!("base64 decode error: {e}"))?,
        ),
        None => None,
    };

    let mut req = client.request(http_method, &url);
    for (name, value) in &headers {
        if super::is_hop_by_hop(name) || name.eq_ignore_ascii_case(TUNNEL_NONCE_HEADER) {
            continue;
        }
        req = req.header(name, value);
    }
    req = req.header(TUNNEL_NONCE_HEADER, tunnel_nonce());
    if let Some(bytes) = decoded_body {
        req = req.body(bytes);
    }
    Ok(req)
}

/// Collect a response's headers into the tunnel's flattened map, dropping
/// hop-by-hop headers and any that aren't valid UTF-8.
fn collect_response_headers(response: &reqwest::Response) -> HashMap<String, String> {
    let mut response_headers = HashMap::new();
    for (name, value) in response.headers() {
        if super::is_hop_by_hop(name.as_str()) {
            continue;
        }
        match value.to_str() {
            Ok(v) => {
                response_headers.insert(name.to_string(), v.to_string());
            }
            Err(_) => {
                debug!(
                    header_name = name.as_str(),
                    "dropping non-UTF8 response header"
                );
            }
        }
    }
    response_headers
}

/// Stream `response`'s body to the relay as `HttpResponseChunk` frames, each
/// at most [`MAX_CHUNK_SIZE`] decoded bytes. Returns the error message for
/// `HttpResponseEnd` when the body stream failed partway through.
async fn stream_body(
    request_id: &str,
    response: reqwest::Response,
    write: &Arc<Mutex<TunnelSink>>,
) -> Option<String> {
    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(e) => {
                warn!(request_id, error = %e, "failed to read A2A response body");
                return Some(format!("failed to read response: {e}"));
            }
        };
        for piece in chunk.chunks(MAX_CHUNK_SIZE) {
            let frame = TunnelFrame::HttpResponseChunk {
                request_id: request_id.to_string(),
                data: STANDARD.encode(piece),
            };
            if let Err(e) = send_frame(write, &frame).await {
                warn!(request_id, error = %e, "failed to send HttpResponseChunk");
                return Some("failed to relay a response chunk to the relay".to_string());
            }
        }
    }
    None
}

/// Send a streamed error response: `HttpResponseStart` with the given status
/// and a plain-text body, then `HttpResponseEnd` with no stream error (the
/// response itself completed fine; it just carries an error status).
pub(super) async fn stream_error(
    write: &Arc<Mutex<TunnelSink>>,
    request_id: &str,
    status: u16,
    message: &str,
) {
    let headers = HashMap::from([(
        "content-type".to_string(),
        "text/plain; charset=utf-8".to_string(),
    )]);
    if let Err(e) = send_frame(
        write,
        &TunnelFrame::HttpResponseStart {
            request_id: request_id.to_string(),
            status,
            headers,
        },
    )
    .await
    {
        warn!(request_id, error = %e, "failed to send HttpResponseStart for error response");
        return;
    }
    if let Err(e) = send_frame(
        write,
        &TunnelFrame::HttpResponseChunk {
            request_id: request_id.to_string(),
            data: STANDARD.encode(message.as_bytes()),
        },
    )
    .await
    {
        warn!(request_id, error = %e, "failed to send HttpResponseChunk for error response");
        return;
    }
    if let Err(e) = send_frame(
        write,
        &TunnelFrame::HttpResponseEnd {
            request_id: request_id.to_string(),
            error: None,
        },
    )
    .await
    {
        warn!(request_id, error = %e, "failed to send HttpResponseEnd for error response");
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::Router;
    use axum::response::sse::{Event, KeepAlive, Sse};
    use axum::routing::get;
    use futures_util::{StreamExt, stream};
    use tokio::net::TcpListener;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    /// Bound on every test's individual waits, so a real regression (a stream
    /// that never sends `HttpResponseEnd`, a cancel that doesn't stop the
    /// upstream request, ...) fails the test instead of hanging the suite.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// A tunnel sink backed by an in-memory loopback WebSocket, so tests can
    /// read back the frames a forward call sends without a real relay.
    async fn loopback_sink() -> (
        Arc<Mutex<TunnelSink>>,
        WebSocketStream<tokio::net::TcpStream>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        });
        let (client_stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let server_stream = accept.await.unwrap();
        let (write, _read) = client_stream.split();
        (Arc::new(Mutex::new(write)), server_stream)
    }

    /// Reads the next tunnel frame, bounded by [`TEST_TIMEOUT`] so a stream
    /// that never sends one fails the test instead of hanging it.
    async fn recv_frame(server: &mut WebSocketStream<tokio::net::TcpStream>) -> TunnelFrame {
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                match server.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("expected a text frame, got {other:?}"),
                }
            }
        })
        .await
        .expect("timed out waiting for a tunnel frame from the forwarder")
    }

    async fn spawn_sse_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/stream",
            get(|| async {
                let events = stream::iter(vec!["one", "two", "three"]).then(|payload| async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok::<_, std::convert::Infallible>(Event::default().data(payload))
                });
                Sse::new(events).keep_alive(KeepAlive::default())
            }),
        );
        spawn_router(app).await
    }

    /// Binds before spawning, so the address is known synchronously and the
    /// caller never blocks waiting on the spawned task — a blocking channel
    /// recv here would deadlock against `#[tokio::test]`'s single-threaded
    /// runtime, since the listener task can't run until this function returns.
    async fn spawn_router(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn streams_start_then_multiple_chunks_then_end() {
        let (addr, server) = spawn_sse_server().await;
        let (write, mut relay) = loopback_sink().await;
        let client = forwarding_client().unwrap();

        tokio::time::timeout(
            TEST_TIMEOUT,
            stream_forward(
                &client,
                addr.port(),
                ForwardRequest {
                    request_id: "req-1".to_string(),
                    method: "GET".to_string(),
                    path: "/stream".to_string(),
                    headers: HashMap::new(),
                    body: None,
                },
                &write,
            ),
        )
        .await
        .expect("stream_forward should not hang for a well-behaved SSE server");
        server.abort();

        let start = recv_frame(&mut relay).await;
        let TunnelFrame::HttpResponseStart { status, .. } = start else {
            panic!("expected HttpResponseStart, got {start:?}");
        };
        assert_eq!(status, 200);

        let mut chunks = 0;
        loop {
            match recv_frame(&mut relay).await {
                TunnelFrame::HttpResponseChunk { .. } => chunks += 1,
                TunnelFrame::HttpResponseEnd { error, .. } => {
                    assert_eq!(error, None, "a clean SSE stream should end without error");
                    break;
                }
                other @ (TunnelFrame::Connected { .. }
                | TunnelFrame::Ping
                | TunnelFrame::Pong
                | TunnelFrame::HttpRequest { .. }
                | TunnelFrame::HttpResponse { .. }
                | TunnelFrame::HttpResponseStart { .. }
                | TunnelFrame::HttpCancel { .. }
                | TunnelFrame::WsOpen { .. }
                | TunnelFrame::WsOpenResult { .. }
                | TunnelFrame::WsMessage { .. }
                | TunnelFrame::WsClose { .. }) => panic!("unexpected frame: {other:?}"),
            }
        }
        assert!(
            chunks >= 3,
            "expected multiple chunks to arrive before the stream ended, got {chunks}"
        );
    }

    #[tokio::test]
    async fn missing_listener_streams_a_502() {
        let (write, mut relay) = loopback_sink().await;
        let client = forwarding_client().unwrap();
        // Nothing is listening on this port.
        let unused_port = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };

        tokio::time::timeout(
            TEST_TIMEOUT,
            stream_forward(
                &client,
                unused_port,
                ForwardRequest {
                    request_id: "req-2".to_string(),
                    method: "GET".to_string(),
                    path: "/".to_string(),
                    headers: HashMap::new(),
                    body: None,
                },
                &write,
            ),
        )
        .await
        .expect("stream_forward should not hang when the local listener refuses the connection");

        let start = recv_frame(&mut relay).await;
        assert!(
            matches!(start, TunnelFrame::HttpResponseStart { status: 502, .. }),
            "expected a streamed 502, got {start:?}"
        );
        let chunk = recv_frame(&mut relay).await;
        assert!(matches!(chunk, TunnelFrame::HttpResponseChunk { .. }));
        let end = recv_frame(&mut relay).await;
        assert!(matches!(
            end,
            TunnelFrame::HttpResponseEnd { error: None, .. }
        ));
    }

    #[tokio::test]
    async fn strips_incoming_nonce_header_and_adds_its_own() {
        let app = Router::new().route(
            "/echo-nonce",
            get(|headers: axum::http::HeaderMap| async move {
                headers
                    .get(TUNNEL_NONCE_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string()
            }),
        );
        let (addr, server) = spawn_router(app).await;
        let (write, mut relay) = loopback_sink().await;
        let client = forwarding_client().unwrap();

        let mut headers = HashMap::new();
        headers.insert(
            TUNNEL_NONCE_HEADER.to_string(),
            "forged-by-caller".to_string(),
        );

        tokio::time::timeout(
            TEST_TIMEOUT,
            stream_forward(
                &client,
                addr.port(),
                ForwardRequest {
                    request_id: "req-3".to_string(),
                    method: "GET".to_string(),
                    path: "/echo-nonce".to_string(),
                    headers,
                    body: None,
                },
                &write,
            ),
        )
        .await
        .expect("stream_forward should not hang for a well-behaved local listener");
        server.abort();

        let _start = recv_frame(&mut relay).await;
        let mut body = Vec::new();
        loop {
            match recv_frame(&mut relay).await {
                TunnelFrame::HttpResponseChunk { data, .. } => {
                    body.extend_from_slice(&STANDARD.decode(data).unwrap());
                }
                TunnelFrame::HttpResponseEnd { .. } => break,
                other @ (TunnelFrame::Connected { .. }
                | TunnelFrame::Ping
                | TunnelFrame::Pong
                | TunnelFrame::HttpRequest { .. }
                | TunnelFrame::HttpResponse { .. }
                | TunnelFrame::HttpResponseStart { .. }
                | TunnelFrame::HttpCancel { .. }
                | TunnelFrame::WsOpen { .. }
                | TunnelFrame::WsOpenResult { .. }
                | TunnelFrame::WsMessage { .. }
                | TunnelFrame::WsClose { .. }) => panic!("unexpected frame: {other:?}"),
            }
        }
        let echoed = String::from_utf8(body).unwrap();
        assert_eq!(
            echoed,
            tunnel_nonce(),
            "the forwarder's own nonce should reach the local listener, not a caller-forged one"
        );
    }
}
