//! WebSocket bus subscriber — translates `BusEvent`s to `ServerMessage` frames.

use tokio::sync::broadcast;

use crate::bus::{BusEvent, Subscriber};
use crate::gateway::protocol::ServerMessage;

/// Receives events from the bus and forwards them as `ServerMessage` frames
/// through the WebSocket broadcast channel.
pub async fn run_ws_subscriber(
    mut subscriber: Subscriber,
    broadcast_tx: broadcast::Sender<ServerMessage>,
) {
    while let Some(event) = subscriber.recv().await {
        let msg = match event {
            BusEvent::TurnStarted { correlation_id } => Some(ServerMessage::TurnStarted {
                reply_to: correlation_id,
            }),
            BusEvent::Response(resp) => Some(ServerMessage::Response {
                reply_to: resp.correlation_id,
                content: resp.content,
            }),
            BusEvent::ToolCall(tc) => Some(ServerMessage::ToolCall {
                id: tc.tool_call_id,
                name: tc.name,
                arguments: tc.arguments,
            }),
            BusEvent::ToolResult(tr) => Some(ServerMessage::ToolResult {
                tool_call_id: tr.tool_call_id,
                name: tr.name,
                output: tr.output,
                is_error: tr.is_error,
            }),
            BusEvent::Intermediate(im) => Some(ServerMessage::BroadcastResponse {
                content: im.content,
            }),
            BusEvent::SystemEvent(se) => Some(ServerMessage::SystemEvent {
                source: se.source,
                content: se.content,
            }),
            // TurnEnded has no ServerMessage equivalent currently — skip
            BusEvent::TurnEnded { .. }
            | BusEvent::Message(_)
            | BusEvent::Notification(_)
            | BusEvent::AgentResult(_)
            | BusEvent::WebhookPayload { .. } => None,
        };

        if let Some(msg) = msg
            && broadcast_tx.send(msg).is_err()
        {
            tracing::trace!("no ws broadcast receivers");
        }
    }

    tracing::debug!("ws subscriber loop ended (broker shut down)");
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::indexing_slicing, reason = "test assertions")]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::{
        IntermediateEvent, ResponseEvent, SystemEventData, ToolCallEvent, ToolResultEvent,
    };

    fn ts() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// Helper: spawns a subscriber loop, publishes events via the bus, and
    /// collects the resulting `ServerMessage`s from the broadcast channel.
    async fn run_subscriber_with_events(events: Vec<BusEvent>) -> Vec<ServerMessage> {
        let bus = crate::bus::spawn_broker();
        let publisher = bus.publisher();
        let subscriber = bus
            .subscribe(crate::bus::TopicId::Interactive(
                crate::bus::EndpointName::from("ws"),
            ))
            .await
            .unwrap();

        let (broadcast_tx, mut broadcast_rx) = broadcast::channel::<ServerMessage>(32);
        let topic = crate::bus::TopicId::Interactive(crate::bus::EndpointName::from("ws"));

        let handle = tokio::spawn(run_ws_subscriber(subscriber, broadcast_tx));

        for event in events {
            publisher.publish(topic.clone(), event).await.unwrap();
        }

        // Give the subscriber loop time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Collect messages
        let mut messages = Vec::new();
        while let Ok(msg) = broadcast_rx.try_recv() {
            messages.push(msg);
        }

        handle.abort();
        messages
    }

    #[tokio::test]
    async fn turn_started_maps_to_server_message() {
        let msgs = run_subscriber_with_events(vec![BusEvent::TurnStarted {
            correlation_id: "c1".into(),
        }])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::TurnStarted { reply_to } if reply_to == "c1"
        ));
    }

    #[tokio::test]
    async fn response_maps_to_server_message() {
        let msgs = run_subscriber_with_events(vec![BusEvent::Response(ResponseEvent {
            correlation_id: "c1".into(),
            content: "hello".into(),
            timestamp: ts(),
        })])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::Response { reply_to, content }
                if reply_to == "c1" && content == "hello"
        ));
    }

    #[tokio::test]
    async fn tool_call_maps_to_server_message() {
        let msgs = run_subscriber_with_events(vec![BusEvent::ToolCall(ToolCallEvent {
            correlation_id: "c1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            arguments: serde_json::json!({"q": "test"}),
        })])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::ToolCall { id, name, .. }
                if id == "tc1" && name == "search"
        ));
    }

    #[tokio::test]
    async fn tool_result_maps_to_server_message() {
        let msgs = run_subscriber_with_events(vec![BusEvent::ToolResult(ToolResultEvent {
            correlation_id: "c1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            output: "found it".into(),
            is_error: false,
        })])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::ToolResult { tool_call_id, name, output, is_error }
                if tool_call_id == "tc1" && name == "search" && output == "found it" && !is_error
        ));
    }

    #[tokio::test]
    async fn intermediate_maps_to_broadcast_response() {
        let msgs = run_subscriber_with_events(vec![BusEvent::Intermediate(IntermediateEvent {
            correlation_id: "c1".into(),
            content: "thinking...".into(),
        })])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::BroadcastResponse { content }
                if content == "thinking..."
        ));
    }

    #[tokio::test]
    async fn system_event_maps_to_server_message() {
        let msgs = run_subscriber_with_events(vec![BusEvent::SystemEvent(SystemEventData {
            correlation_id: "c1".into(),
            source: "pulse".into(),
            content: "check done".into(),
            timestamp: ts(),
        })])
        .await;

        assert_eq!(msgs.len(), 1);
        assert!(matches!(
            &msgs[0],
            ServerMessage::SystemEvent { source, content }
                if source == "pulse" && content == "check done"
        ));
    }

    #[tokio::test]
    async fn turn_ended_is_skipped() {
        let msgs = run_subscriber_with_events(vec![BusEvent::TurnEnded {
            correlation_id: "c1".into(),
        }])
        .await;

        assert!(
            msgs.is_empty(),
            "TurnEnded should not produce a ServerMessage"
        );
    }
}
