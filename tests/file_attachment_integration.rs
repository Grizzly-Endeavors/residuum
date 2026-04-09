//! Integration test: file attachment through `send_message` → bus → subscriber.
//!
//! Tests the full backend flow: `send_message` tool with `file_path` →
//! `ResponseEvent` with `FileAttachment` → WebSocket subscriber maps to
//! `ServerMessage::FileAttachment` with a valid URL.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::expect_used, reason = "test code uses expect for clarity")]
#[expect(
    clippy::panic,
    reason = "test assertions use panic on unexpected variants"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test assertions use wildcard for non-matching variants"
)]
#[cfg(test)]
mod tests {
    use residuum::bus::{
        EndpointCapabilities, EndpointEntry, EndpointId, EndpointName, EndpointRegistry,
        ResponseEvent, TopicId, topics,
    };
    use residuum::gateway::file_server::FileRegistry;
    use residuum::gateway::protocol::ServerMessage;
    use residuum::interfaces::websocket::subscriber::WsSubscribers;
    use residuum::tools::Tool;
    use residuum::tools::send_message::SendMessageTool;

    fn make_registry() -> EndpointRegistry {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "WebSocket".to_string(),
        });
        registry
    }

    #[tokio::test]
    async fn send_message_file_publishes_response_event_with_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("report.pdf");
        tokio::fs::write(&file_path, b"fake pdf content")
            .await
            .unwrap();

        let bus = residuum::bus::spawn_broker();
        let publisher = bus.publisher();
        let mut subscriber = bus
            .subscribe(topics::Endpoint(EndpointName::from("ws")))
            .await
            .unwrap();

        let registry = make_registry();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "file_path": file_path.to_str().unwrap()
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "tool should succeed: {}", result.output);
        assert!(
            result.output.contains("report.pdf"),
            "success message should contain filename: {}",
            result.output
        );

        let event: ResponseEvent = subscriber.recv().await.unwrap().unwrap();
        let att = event
            .attachment
            .expect("ResponseEvent should have attachment");
        assert_eq!(att.filename, "report.pdf");
        assert_eq!(att.mime_type, "application/pdf");
        assert_eq!(att.size, 16); // b"fake pdf content".len()
    }

    #[tokio::test]
    async fn ws_subscriber_maps_response_event_to_file_attachment_message() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        tokio::fs::write(&file_path, b"fake image bytes")
            .await
            .unwrap();

        let bus = residuum::bus::spawn_broker();
        let publisher = bus.publisher();
        let ep = EndpointName::from("ws");
        let file_registry = FileRegistry::new();
        let mut subs = WsSubscribers::new(&bus, ep.clone(), file_registry)
            .await
            .unwrap();

        let att = residuum::interfaces::attachment::FileAttachment {
            path: file_path.clone(),
            filename: "photo.jpg".to_string(),
            mime_type: "image/jpeg".to_string(),
            size: 16,
        };

        publisher
            .publish(
                topics::Endpoint(ep),
                ResponseEvent {
                    correlation_id: "req-1".into(),
                    content: "Here's your photo".into(),
                    timestamp: chrono::Utc::now().naive_utc(),
                    attachment: Some(att),
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        match msg {
            ServerMessage::FileAttachment {
                reply_to,
                filename,
                mime_type,
                size,
                url,
                caption,
            } => {
                assert_eq!(reply_to, "req-1");
                assert_eq!(filename, "photo.jpg");
                assert_eq!(mime_type, "image/jpeg");
                assert_eq!(size, 16);
                assert!(
                    url.starts_with("/api/files/"),
                    "url should be a file API path: {url}"
                );
                assert_eq!(caption, Some("Here's your photo".to_string()));
            }
            other => panic!("expected FileAttachment, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_message_text_only_still_works() {
        let bus = residuum::bus::spawn_broker();
        let publisher = bus.publisher();
        let mut subscriber = bus
            .subscribe(topics::Endpoint(EndpointName::from("ws")))
            .await
            .unwrap();

        let registry = make_registry();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "message": "hello from agent"
            }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "text-only should succeed: {}",
            result.output
        );

        let event: ResponseEvent = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(event.content, "hello from agent");
        assert!(
            event.attachment.is_none(),
            "text-only should have no attachment"
        );
    }
}
