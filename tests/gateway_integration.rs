//! End-to-end integration tests for the WebSocket gateway.
//!
//! Tests the gateway server using mock providers, verifying protocol
//! behavior including ping/pong, message round-trip, verbose filtering,
//! multi-client broadcast, and error handling.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::expect_used, reason = "test code uses expect for clarity")]
#[expect(
    clippy::panic,
    reason = "test assertions use panic on unexpected variants"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod gateway_integration {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::{broadcast, mpsc};
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    use residuum::agent::Agent;
    use residuum::agent::context::PromptContext;
    use residuum::agent::interrupt;
    use residuum::bus::{EndpointName, TopicId, spawn_broker};
    use residuum::gateway::protocol::{ClientMessage, ServerMessage};
    use residuum::models::{
        CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ToolDefinition,
    };
    use residuum::tools::{ToolFilter, ToolRegistry};
    use residuum::workspace::identity::IdentityFiles;

    /// Mock provider that returns configurable responses in sequence.
    struct MockProvider {
        responses: Vec<String>,
        call_idx: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_idx: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
            let content = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| self.responses.last().cloned().unwrap_or_default());
            Ok(ModelResponse::new(content, vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-gateway"
        }
    }

    fn make_agent(responses: Vec<String>) -> Agent {
        Agent::new(
            Box::new(MockProvider::new(responses)),
            ToolRegistry::new(),
            ToolFilter::new_shared(std::collections::HashSet::new()),
            residuum::mcp::McpRegistry::new_shared(),
            IdentityFiles::default(),
            residuum::agent::AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        )
    }

    /// Message that flows from a WebSocket client into the main loop.
    struct InboundMessage {
        id: String,
        content: String,
    }

    /// Start a minimal gateway test harness and return the channels and bound address.
    async fn start_test_gateway(
        agent: Agent,
    ) -> (
        mpsc::Sender<InboundMessage>,
        broadcast::Sender<ServerMessage>,
        String,
    ) {
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(32);
        let (broadcast_tx, _) = broadcast::channel::<ServerMessage>(256);

        // Set up bus broker and wire the ws subscriber loop to forward
        // bus events to the broadcast channel.
        let bus = spawn_broker();
        let publisher = bus.publisher();
        let output_topic = TopicId::Interactive(EndpointName::from("ws"));

        let sub = bus.subscribe(output_topic.clone()).await.unwrap();
        let sub_broadcast = broadcast_tx.clone();
        tokio::spawn(
            residuum::interfaces::websocket::subscriber::run_ws_subscriber(sub, sub_broadcast),
        );

        let state = TestGatewayState {
            inbound_tx: inbound_tx.clone(),
            broadcast_tx: broadcast_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(test_ws_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let loop_broadcast_tx = broadcast_tx.clone();
        let loop_publisher = publisher.clone();
        let loop_topic = output_topic.clone();
        tokio::spawn(async move {
            let mut agent = agent;
            while let Some(inbound) = inbound_rx.recv().await {
                let reply_id = inbound.id.clone();

                // Publish TurnStarted (normally done by the event loop)
                drop(
                    loop_publisher
                        .publish(
                            loop_topic.clone(),
                            residuum::bus::BusEvent::TurnStarted {
                                correlation_id: reply_id.clone(),
                            },
                        )
                        .await,
                );

                let mut irx = interrupt::dead_interrupt_rx();
                match agent
                    .process_message(
                        &inbound.content,
                        &loop_publisher,
                        &loop_topic,
                        None,
                        &PromptContext::none(),
                        &mut irx,
                        &[],
                    )
                    .await
                {
                    Ok(texts) => {
                        for text in &texts {
                            drop(
                                loop_publisher
                                    .publish(
                                        loop_topic.clone(),
                                        residuum::bus::BusEvent::Response(
                                            residuum::bus::ResponseEvent {
                                                correlation_id: reply_id.clone(),
                                                content: text.clone(),
                                                timestamp: chrono::NaiveDateTime::default(),
                                            },
                                        ),
                                    )
                                    .await,
                            );
                        }
                    }
                    Err(e) => {
                        if loop_broadcast_tx
                            .send(ServerMessage::Error {
                                reply_to: Some(reply_id),
                                message: e.to_string(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        (inbound_tx, broadcast_tx, addr)
    }

    #[derive(Clone)]
    struct TestGatewayState {
        inbound_tx: mpsc::Sender<InboundMessage>,
        broadcast_tx: broadcast::Sender<ServerMessage>,
    }

    async fn test_ws_handler(
        ws: axum::extract::WebSocketUpgrade,
        axum::extract::State(state): axum::extract::State<TestGatewayState>,
    ) -> impl axum::response::IntoResponse {
        ws.on_upgrade(|socket| test_handle_connection(socket, state))
    }

    async fn test_handle_connection(socket: axum::extract::ws::WebSocket, state: TestGatewayState) {
        use axum::extract::ws::Message as WsMessage;

        let (mut ws_tx, mut ws_rx) = socket.split();

        let mut broadcast_rx = state.broadcast_tx.subscribe();

        // Forward all messages to the client (verbose filtering is client-side)
        let fwd_handle = tokio::spawn(async move {
            while let Ok(msg) = broadcast_rx.recv().await {
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };
                if ws_tx.send(WsMessage::text(json)).await.is_err() {
                    break;
                }
            }
        });

        while let Some(frame) = ws_rx.next().await {
            let raw = match frame {
                Ok(WsMessage::Text(txt)) => txt,
                Ok(WsMessage::Close(_)) | Err(_) => break,
                Ok(WsMessage::Binary(_) | WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
            };

            let client_msg: ClientMessage = match serde_json::from_str(&raw) {
                Ok(m) => m,
                Err(e) => {
                    if state
                        .broadcast_tx
                        .send(ServerMessage::Error {
                            reply_to: None,
                            message: format!("malformed message: {e}"),
                        })
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
            };

            match client_msg {
                ClientMessage::SendMessage { id, content, .. } => {
                    if state
                        .inbound_tx
                        .send(InboundMessage { id, content })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                ClientMessage::Ping => {
                    if state.broadcast_tx.send(ServerMessage::Pong).is_err() {
                        break;
                    }
                }
                // SetVerbose (client-side), Reload, ServerCommand, InboxAdd
                // not handled in test stub
                ClientMessage::SetVerbose { .. }
                | ClientMessage::Reload
                | ClientMessage::ServerCommand { .. }
                | ClientMessage::InboxAdd { .. } => {}
            }
        }

        fwd_handle.abort();
    }

    /// Connect a WebSocket client to the test gateway.
    async fn connect_client(
        addr: &str,
    ) -> (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            TungsteniteMessage,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        let url = format!("ws://{addr}/ws");
        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        ws_stream.split()
    }

    /// Send a `ClientMessage` over the WS connection.
    async fn send_msg(
        tx: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            TungsteniteMessage,
        >,
        msg: &ClientMessage,
    ) {
        let json = serde_json::to_string(msg).unwrap();
        tx.send(TungsteniteMessage::text(json)).await.unwrap();
    }

    /// Receive the next `ServerMessage` from the WS connection (with timeout).
    async fn recv_msg(
        rx: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> ServerMessage {
        let timeout = tokio::time::Duration::from_secs(5);
        let frame = tokio::time::timeout(timeout, rx.next())
            .await
            .expect("timed out waiting for server message")
            .expect("stream ended unexpectedly");

        match frame {
            Ok(TungsteniteMessage::Text(txt)) => serde_json::from_str(&txt).unwrap(),
            Ok(
                TungsteniteMessage::Binary(_)
                | TungsteniteMessage::Ping(_)
                | TungsteniteMessage::Pong(_)
                | TungsteniteMessage::Close(_)
                | TungsteniteMessage::Frame(_),
            ) => panic!("expected text frame, got non-text"),
            Err(e) => panic!("websocket error in recv_msg: {e}"),
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn ping_pong() {
        let agent = make_agent(vec!["hello".to_string()]);
        let (_inbound_tx, _broadcast_tx, addr) = start_test_gateway(agent).await;

        let (mut tx, mut rx) = connect_client(&addr).await;

        send_msg(&mut tx, &ClientMessage::Ping).await;
        let msg = recv_msg(&mut rx).await;
        assert!(
            matches!(msg, ServerMessage::Pong),
            "should receive Pong, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_message_and_receive_response() {
        let agent = make_agent(vec!["hello back!".to_string()]);
        let (_inbound_tx, _broadcast_tx, addr) = start_test_gateway(agent).await;

        let (mut tx, mut rx) = connect_client(&addr).await;

        send_msg(
            &mut tx,
            &ClientMessage::SendMessage {
                id: "msg-1".to_string(),
                content: "hello".to_string(),
                images: vec![],
            },
        )
        .await;

        let msg1 = recv_msg(&mut rx).await;
        assert!(
            matches!(&msg1, ServerMessage::TurnStarted { reply_to } if reply_to == "msg-1"),
            "should receive TurnStarted, got: {msg1:?}"
        );

        let msg2 = recv_msg(&mut rx).await;
        assert!(
            matches!(
                &msg2,
                ServerMessage::Response { reply_to, content }
                    if reply_to == "msg-1" && content == "hello back!"
            ),
            "should receive Response with correct fields, got: {msg2:?}"
        );
    }

    #[tokio::test]
    async fn all_clients_receive_tool_events() {
        let agent = make_agent(vec!["done".to_string()]);
        let (_inbound_tx, broadcast_tx, addr) = start_test_gateway(agent).await;

        let (_tx, mut rx) = connect_client(&addr).await;

        // All clients receive tool events regardless of verbose setting
        // (verbose filtering is handled client-side)
        assert!(
            broadcast_tx
                .send(ServerMessage::ToolCall {
                    id: "tc-1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                })
                .is_ok(),
            "broadcast send should succeed"
        );

        let msg = recv_msg(&mut rx).await;
        assert!(
            matches!(&msg, ServerMessage::ToolCall { .. }),
            "client should receive ToolCall (filtering is client-side), got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn all_clients_receive_tool_results() {
        let agent = make_agent(vec!["done".to_string()]);
        let (_inbound_tx, broadcast_tx, addr) = start_test_gateway(agent).await;

        let (_tx, mut rx) = connect_client(&addr).await;

        assert!(
            broadcast_tx
                .send(ServerMessage::ToolResult {
                    tool_call_id: "tc-1".to_string(),
                    name: "exec".to_string(),
                    output: "test output".to_string(),
                    is_error: false,
                })
                .is_ok(),
            "broadcast send should succeed"
        );

        let msg = recv_msg(&mut rx).await;
        assert!(
            matches!(&msg, ServerMessage::ToolResult { .. }),
            "client should receive ToolResult (filtering is client-side), got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn multiple_clients_receive_broadcast() {
        let agent = make_agent(vec!["shared response".to_string()]);
        let (_inbound_tx, _broadcast_tx, addr) = start_test_gateway(agent).await;

        let (mut tx_a, mut rx_a) = connect_client(&addr).await;
        let (_tx_b, mut rx_b) = connect_client(&addr).await;

        send_msg(
            &mut tx_a,
            &ClientMessage::SendMessage {
                id: "multi-1".to_string(),
                content: "hello from A".to_string(),
                images: vec![],
            },
        )
        .await;

        let a1 = recv_msg(&mut rx_a).await;
        let b1 = recv_msg(&mut rx_b).await;
        assert!(
            matches!(&a1, ServerMessage::TurnStarted { .. }),
            "client A should receive TurnStarted"
        );
        assert!(
            matches!(&b1, ServerMessage::TurnStarted { .. }),
            "client B should receive TurnStarted"
        );

        let a2 = recv_msg(&mut rx_a).await;
        let b2 = recv_msg(&mut rx_b).await;
        assert!(
            matches!(&a2, ServerMessage::Response { content, .. } if content == "shared response"),
            "client A should receive Response"
        );
        assert!(
            matches!(&b2, ServerMessage::Response { content, .. } if content == "shared response"),
            "client B should receive Response"
        );
    }

    #[tokio::test]
    async fn malformed_json_returns_error() {
        let agent = make_agent(vec!["ok".to_string()]);
        let (_inbound_tx, _broadcast_tx, addr) = start_test_gateway(agent).await;

        let (mut tx, mut rx) = connect_client(&addr).await;

        tx.send(TungsteniteMessage::text("{not valid json}"))
            .await
            .unwrap();

        let msg = recv_msg(&mut rx).await;
        assert!(
            matches!(
                &msg,
                ServerMessage::Error { reply_to, message }
                    if reply_to.is_none() && message.contains("malformed")
            ),
            "should receive Error with 'malformed' message, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn client_disconnect_does_not_crash_gateway() {
        let agent = make_agent(vec![
            "first response".to_string(),
            "second response".to_string(),
        ]);
        let (_inbound_tx, _broadcast_tx, addr) = start_test_gateway(agent).await;

        // Connect and disconnect client A
        {
            let (mut tx_a, mut rx_a) = connect_client(&addr).await;
            send_msg(
                &mut tx_a,
                &ClientMessage::SendMessage {
                    id: "d-1".to_string(),
                    content: "before disconnect".to_string(),
                    images: vec![],
                },
            )
            .await;

            let _ = recv_msg(&mut rx_a).await; // TurnStarted
            let _ = recv_msg(&mut rx_a).await; // Response
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Client B should still work
        let (mut tx_b, mut rx_b) = connect_client(&addr).await;
        send_msg(
            &mut tx_b,
            &ClientMessage::SendMessage {
                id: "d-2".to_string(),
                content: "after disconnect".to_string(),
                images: vec![],
            },
        )
        .await;

        let msg1 = recv_msg(&mut rx_b).await;
        assert!(
            matches!(&msg1, ServerMessage::TurnStarted { .. }),
            "should receive TurnStarted after other client disconnected"
        );

        let msg2 = recv_msg(&mut rx_b).await;
        assert!(
            matches!(&msg2, ServerMessage::Response { .. }),
            "should receive Response after other client disconnected"
        );
    }
}
