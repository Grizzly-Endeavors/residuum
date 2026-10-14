//! What a running agent's socket says about a settings reload: every outcome
//! of a reload of its own config and of the hub's carries a `reload` kind the
//! web words its toast from.

use super::*;

type AgentSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Read the agent's socket up to the reload notice whose text `matches`, and
/// return that notice's `reload` kind. Every reload notice read on the way
/// must carry a kind too.
async fn reload_kind_of(ws: &mut AgentSocket, matches: impl Fn(&str) -> bool) -> String {
    wait::guarded("the reload notice", async {
        while let Some(frame) = ws.next().await {
            let WsMessage::Text(raw) = frame.unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&raw).unwrap();
            if value.get("type").and_then(Value::as_str) != Some("notice") {
                continue;
            }
            let message = str_at(&value, "message");
            if !message.contains("reload") {
                continue;
            }
            let kind = value
                .get("reload")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a reload notice without its kind: {value}"))
                .to_string();
            if matches(message) {
                return kind;
            }
        }
        panic!("the agent's socket closed before the reload notice");
    })
    .await
}

async fn ask_for_reload(ws: &mut AgentSocket) {
    ws.send(WsMessage::text(json!({ "type": "reload" }).to_string()))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_agent_config_reload_reports_each_outcome_by_kind() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    let config_toml = hub
        .root
        .path()
        .join("scout")
        .join("config")
        .join("config.toml");

    ask_for_reload(&mut ws).await;
    assert_eq!(
        reload_kind_of(&mut ws, |m| m
            == "configuration reloaded: no changes detected")
        .await,
        "unchanged"
    );

    std::fs::write(&config_toml, "timeout_secs = 90\n").unwrap();
    ask_for_reload(&mut ws).await;
    assert_eq!(
        reload_kind_of(&mut ws, |m| m
            .starts_with("configuration reloaded: http timeout"))
        .await,
        "reloaded"
    );

    std::fs::write(&config_toml, "not valid toml [[[").unwrap();
    ask_for_reload(&mut ws).await;
    assert_eq!(
        reload_kind_of(&mut ws, |m| m.starts_with("config reload failed")).await,
        "failed"
    );
}

#[tokio::test]
async fn a_hub_config_reload_reports_each_outcome_on_the_agent_socket_by_kind() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    // A reload answered on this socket shows the socket hears the agent's
    // notices before the hub's reloads below are sent.
    ask_for_reload(&mut ws).await;
    reload_kind_of(&mut ws, |m| {
        m == "configuration reloaded: no changes detected"
    })
    .await;

    let hub_dir = hub.root.path().join("hub");
    std::fs::write(
        hub_dir.join("config.toml"),
        "timezone = \"America/Chicago\"\n",
    )
    .unwrap();
    let chicago = HubConfig::load_at(&hub_dir).unwrap();
    hub.host.hub_config_changed(chicago.clone());
    assert_eq!(
        reload_kind_of(&mut ws, |m| m
            .starts_with("hub configuration reloaded: timezone"))
        .await,
        "reloaded"
    );

    hub.host.hub_config_changed(chicago.clone());
    assert_eq!(
        reload_kind_of(&mut ws, |m| m
            == "hub configuration reloaded: no changes detected")
        .await,
        "unchanged"
    );

    std::fs::write(hub_dir.join("config.toml"), "not valid toml [[[").unwrap();
    hub.host.hub_config_changed(chicago);
    assert_eq!(
        reload_kind_of(&mut ws, |m| m.starts_with("hub config reload failed")).await,
        "failed"
    );
}
