//! The artifacts listener in front of a real hub: API calls and sockets
//! through its port, and what a socket opened there does not count as.

use axum::http::HeaderValue;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::*;
use crate::hub::agent_watch::{AgentChangeKind, AgentChangeReceiver, MainTurnEnded};
use crate::workbench::forward::HubApi;

type PageSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serve the artifacts listener over a hub router built on `hub`, and return
/// its address.
async fn artifacts_port(hub: &Fixture) -> String {
    let (reload_tx, _reload_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = build_app(&hub.host, &hub.services, reload_tx, None).unwrap();
    let api = HubApi::new();
    api.bind(app);
    let router = crate::workbench::server::router(hub.root.path().join("team/workbench"), api);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    crate::util::spawn_in_span(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

async fn connect(addr: &str, path: &str) -> PageSocket {
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
        .await
        .unwrap();
    socket
}

async fn send_frame(socket: &mut PageSocket, frame: &Value) {
    socket
        .send(WsMessage::text(frame.to_string()))
        .await
        .unwrap();
}

/// Read frames until one of type `kind` arrives, and return it.
async fn next_frame_of(socket: &mut PageSocket, kind: &str) -> Value {
    tokio::time::timeout(POLL_TIMEOUT, async {
        while let Some(frame) = socket.next().await {
            let WsMessage::Text(raw) = frame.unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&raw).unwrap();
            if value.get("type") == Some(&json!(kind)) {
                return value;
            }
        }
        panic!("the socket closed before a {kind} frame arrived");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for a {kind} frame"))
}

/// The next main turn the feed reports as ended.
async fn next_turn(changes: &mut AgentChangeReceiver) -> MainTurnEnded {
    tokio::time::timeout(POLL_TIMEOUT, async {
        loop {
            let change = changes.recv().await.expect("the feed stays open");
            if let AgentChangeKind::TurnEnded(turn) = change.kind {
                return turn;
            }
        }
    })
    .await
    .expect("a turn ends within the timeout")
}

#[tokio::test]
async fn the_api_and_the_hub_socket_answer_on_the_artifacts_port() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let artifacts = artifacts_port(&hub).await;
    let http = reqwest::Client::new();
    let url = |path: &str| format!("http://{artifacts}{path}");

    // The hub's routes and one agent's, resolved per request.
    let agents = http.get(url("/api/hub/agents")).send().await.unwrap();
    assert_eq!(agents.status().as_u16(), 200);
    let listing: Value = agents.json().await.unwrap();
    assert_eq!(
        str_at(array_at(&listing, "agents").first().unwrap(), "name"),
        "scout"
    );
    let status = http
        .get(url("/api/agents/scout/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(status.status().as_u16(), 200);

    // The hub socket.
    let mut socket = connect(&artifacts, "/api/hub/ws").await;
    let boot = next_frame_of(&mut socket, "hub_boot").await;
    assert!(boot.get("boot_id").is_some(), "{boot}");

    // A route artifacts can't use is refused and does nothing.
    let refused = http.post(url("/api/hub/stop-all")).send().await.unwrap();
    assert_eq!(refused.status().as_u16(), 403);
    let refusal: Value = refused.json().await.unwrap();
    assert!(refusal.get("error").is_some(), "{refusal}");
    assert_eq!(hub.state_of("scout"), AgentState::Running);
}

#[tokio::test]
async fn only_the_api_is_served_from_the_hub_router_on_the_artifacts_port() {
    let hub = Fixture::new(&["scout"], "").await;
    let artifacts = artifacts_port(&hub).await;
    let http = reqwest::Client::new();

    // An unknown API path is the hub's 404, never the embedded web app.
    let missing = http
        .get(format!("http://{artifacts}/api/no-such-route"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
    assert!(!missing.text().await.unwrap().contains("<html"));

    // The root is the listener's own page, not the web UI.
    let root = http
        .get(format!("http://{artifacts}/"))
        .send()
        .await
        .unwrap();
    let page = root.text().await.unwrap();
    assert!(page.contains("Workbench artifacts open from the Workbench page"));
}

#[tokio::test]
async fn another_sites_requests_are_refused_on_the_artifacts_port() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let artifacts = artifacts_port(&hub).await;

    let write = reqwest::Client::new()
        .post(format!("http://{artifacts}/api/hub/agents/scout/stop"))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(write.status().as_u16(), 403);
    assert_eq!(hub.state_of("scout"), AgentState::Running);

    let mut upgrade = format!("ws://{artifacts}/api/hub/ws")
        .into_client_request()
        .unwrap();
    upgrade
        .headers_mut()
        .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    let refused = tokio_tungstenite::connect_async(upgrade).await;
    assert!(
        matches!(
            &refused,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 403
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_agent_socket_through_the_artifacts_port_does_not_reset_unread() {
    let hub = Fixture::new(&["scout"], "").await;
    // A slow model, so the client can leave before the reply is published.
    hub.mock("scout").reset().await;
    mount_reply(hub.mock("scout"), "scout here", Duration::from_millis(600)).await;
    hub.host.start("scout").await.unwrap();
    let artifacts = artifacts_port(&hub).await;

    let mut ui = connect(&hub.addr, "/api/agents/scout/ws").await;
    send_frame(
        &mut ui,
        &json!({ "type": "send_message", "id": "m1", "content": "ping" }),
    )
    .await;
    eventually("scout to be busy", || async {
        hub.activity_of("scout").busy.then_some(())
    })
    .await;
    ui.close(None).await.unwrap();
    drop(ui);
    eventually("the unread reply", || async {
        let activity = hub.activity_of("scout");
        (!activity.busy && activity.unread == 1).then_some(())
    })
    .await;

    // The pong proves the connection is past the point where a client is counted.
    let mut page = connect(&artifacts, "/api/agents/scout/ws").await;
    send_frame(&mut page, &json!({ "type": "ping" })).await;
    next_frame_of(&mut page, "pong").await;
    assert_eq!(
        hub.activity_of("scout").unread,
        1,
        "a workbench page's socket isn't the user reading the chat"
    );

    // A client of the web UI still does.
    let _ui = connect(&hub.addr, "/api/agents/scout/ws").await;
    eventually("the unread count to reset", || async {
        (hub.activity_of("scout").unread == 0).then_some(())
    })
    .await;
}

#[tokio::test]
async fn an_agent_socket_through_the_artifacts_port_is_not_a_connected_client() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let artifacts = artifacts_port(&hub).await;
    let mut changes = hub.host.agent_changes().subscribe();

    // A turn with only a workbench page's socket open has no client.
    let mut page = connect(&artifacts, "/api/agents/scout/ws").await;
    send_frame(
        &mut page,
        &json!({ "type": "send_message", "id": "m1", "content": "hello" }),
    )
    .await;
    next_frame_of(&mut page, "response").await;
    let page_turn = next_turn(&mut changes).await;
    assert!(!page_turn.client_connected);

    // With the web UI's socket open too, it does.
    let mut ui = connect(&hub.addr, "/api/agents/scout/ws").await;
    send_frame(
        &mut ui,
        &json!({ "type": "send_message", "id": "m2", "content": "hello again" }),
    )
    .await;
    next_frame_of(&mut ui, "response").await;
    let ui_turn = next_turn(&mut changes).await;
    assert!(ui_turn.client_connected);
}
