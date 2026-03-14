//! WebSocket bus subscriber — translates `BusEvent`s to `ServerMessage` frames.

use crate::bus::BusEvent;
use crate::gateway::protocol::ServerMessage;

/// Translate a `BusEvent` into a `ServerMessage`, if applicable.
///
/// Returns `None` for events that have no WebSocket representation (e.g.
/// `TurnEnded`, `Message`, `Notification`).
#[must_use]
pub fn translate_bus_event(event: BusEvent) -> Option<ServerMessage> {
    match event {
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
        BusEvent::Error {
            correlation_id,
            message,
        } => Some(ServerMessage::Error {
            reply_to: Some(correlation_id),
            message,
        }),
        BusEvent::Notice { message } => Some(ServerMessage::Notice { message }),
        // TurnEnded has no ServerMessage equivalent currently — skip
        BusEvent::TurnEnded { .. }
        | BusEvent::Message(_)
        | BusEvent::Notification(_)
        | BusEvent::AgentResult(_)
        | BusEvent::WebhookPayload { .. }
        | BusEvent::SpawnRequest(_) => None,
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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

    #[test]
    fn turn_started_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::TurnStarted {
            correlation_id: "c1".into(),
        });
        assert!(matches!(
            msg,
            Some(ServerMessage::TurnStarted { reply_to }) if reply_to == "c1"
        ));
    }

    #[test]
    fn response_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::Response(ResponseEvent {
            correlation_id: "c1".into(),
            content: "hello".into(),
            timestamp: ts(),
        }));
        assert!(matches!(
            msg,
            Some(ServerMessage::Response { reply_to, content })
                if reply_to == "c1" && content == "hello"
        ));
    }

    #[test]
    fn tool_call_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::ToolCall(ToolCallEvent {
            correlation_id: "c1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            arguments: serde_json::json!({"q": "test"}),
        }));
        assert!(matches!(
            msg,
            Some(ServerMessage::ToolCall { id, name, .. })
                if id == "tc1" && name == "search"
        ));
    }

    #[test]
    fn tool_result_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::ToolResult(ToolResultEvent {
            correlation_id: "c1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            output: "found it".into(),
            is_error: false,
        }));
        assert!(matches!(
            msg,
            Some(ServerMessage::ToolResult { tool_call_id, name, output, is_error })
                if tool_call_id == "tc1" && name == "search" && output == "found it" && !is_error
        ));
    }

    #[test]
    fn intermediate_maps_to_broadcast_response() {
        let msg = translate_bus_event(BusEvent::Intermediate(IntermediateEvent {
            correlation_id: "c1".into(),
            content: "thinking...".into(),
        }));
        assert!(matches!(
            msg,
            Some(ServerMessage::BroadcastResponse { content })
                if content == "thinking..."
        ));
    }

    #[test]
    fn system_event_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::SystemEvent(SystemEventData {
            correlation_id: "c1".into(),
            source: "pulse".into(),
            content: "check done".into(),
            timestamp: ts(),
        }));
        assert!(matches!(
            msg,
            Some(ServerMessage::SystemEvent { source, content })
                if source == "pulse" && content == "check done"
        ));
    }

    #[test]
    fn error_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::Error {
            correlation_id: "c1".into(),
            message: "something broke".into(),
        });
        assert!(matches!(
            msg,
            Some(ServerMessage::Error { reply_to, message })
                if reply_to == Some("c1".to_string()) && message == "something broke"
        ));
    }

    #[test]
    fn notice_maps_to_server_message() {
        let msg = translate_bus_event(BusEvent::Notice {
            message: "reloading".into(),
        });
        assert!(matches!(
            msg,
            Some(ServerMessage::Notice { message })
                if message == "reloading"
        ));
    }

    #[test]
    fn turn_ended_is_skipped() {
        let msg = translate_bus_event(BusEvent::TurnEnded {
            correlation_id: "c1".into(),
        });
        assert!(
            msg.is_none(),
            "TurnEnded should not produce a ServerMessage"
        );
    }
}
