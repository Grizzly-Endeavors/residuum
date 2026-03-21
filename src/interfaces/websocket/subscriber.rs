//! WebSocket bus subscriber — translates typed bus events to `ServerMessage` frames.

use crate::bus::{
    EndpointName, ErrorEvent, IntermediateEvent, NoticeEvent, NotifyName, ResponseEvent,
    Subscriber, ToolActivityEvent, TurnLifecycleEvent, topics,
};
use crate::gateway::protocol::ServerMessage;

/// Typed subscribers for a single WebSocket connection.
pub struct WsSubscribers {
    pub response: Subscriber<ResponseEvent>,
    pub tool_activity: Subscriber<ToolActivityEvent>,
    pub turn_lifecycle: Subscriber<TurnLifecycleEvent>,
    pub intermediate: Subscriber<IntermediateEvent>,
    pub notice: Subscriber<NoticeEvent>,
    pub error: Subscriber<ErrorEvent>,
}

impl WsSubscribers {
    /// Create all typed subscribers for a WebSocket connection.
    ///
    /// # Errors
    ///
    /// Returns `BusError` if any subscription fails.
    pub async fn new(
        bus_handle: &crate::bus::BusHandle,
        ep: EndpointName,
    ) -> Result<Self, crate::bus::BusError> {
        let system_topic = || topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL));
        Ok(Self {
            response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            tool_activity: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            turn_lifecycle: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            intermediate: bus_handle.subscribe(topics::Endpoint(ep)).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
        })
    }

    /// Receive the next server message from any subscribed topic.
    ///
    /// Returns `None` when all subscribers have closed.
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        loop {
            let msg = tokio::select! {
                event = self.response.recv() => {
                    match event {
                        Ok(Some(resp)) => Some(ServerMessage::Response {
                            reply_to: resp.correlation_id,
                            content: resp.content,
                        }),
                        _ => return None,
                    }
                }
                event = self.tool_activity.recv() => {
                    match event {
                        Ok(Some(ToolActivityEvent::Call(tc))) => Some(ServerMessage::ToolCall {
                            id: tc.tool_call_id,
                            name: tc.name,
                            arguments: tc.arguments,
                        }),
                        Ok(Some(ToolActivityEvent::Result(tr))) => Some(ServerMessage::ToolResult {
                            tool_call_id: tr.tool_call_id,
                            name: tr.name,
                            output: tr.output,
                            is_error: tr.is_error,
                        }),
                        _ => return None,
                    }
                }
                event = self.turn_lifecycle.recv() => {
                    match event {
                        Ok(Some(TurnLifecycleEvent::Started { correlation_id })) => {
                            Some(ServerMessage::TurnStarted { reply_to: correlation_id })
                        }
                        Ok(Some(TurnLifecycleEvent::Ended { .. })) => {
                            // TurnEnded has no ServerMessage equivalent currently — skip
                            continue;
                        }
                        _ => return None,
                    }
                }
                event = self.intermediate.recv() => {
                    match event {
                        Ok(Some(im)) => Some(ServerMessage::BroadcastResponse {
                            content: im.content,
                        }),
                        _ => return None,
                    }
                }
                event = self.notice.recv() => {
                    match event {
                        Ok(Some(NoticeEvent { message })) => {
                            Some(ServerMessage::Notice { message })
                        }
                        _ => return None,
                    }
                }
                event = self.error.recv() => {
                    match event {
                        Ok(Some(ErrorEvent { correlation_id, message })) => {
                            Some(ServerMessage::Error {
                                reply_to: Some(correlation_id),
                                message,
                            })
                        }
                        _ => return None,
                    }
                }
            };

            if let Some(msg) = msg {
                return Some(msg);
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::{
        IntermediateEvent, NotifyName, ResponseEvent, ToolCallEvent, ToolResultEvent,
    };

    fn ts() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn response_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(&handle, ep.clone()).await.unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ResponseEvent {
                correlation_id: "c1".into(),
                content: "hello".into(),
                timestamp: ts(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Response { reply_to, content }
                if reply_to == "c1" && content == "hello"
        ));
    }

    #[tokio::test]
    async fn tool_call_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(&handle, ep.clone()).await.unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ToolActivityEvent::Call(ToolCallEvent {
                correlation_id: "c1".into(),
                tool_call_id: "tc1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"q": "test"}),
            }),
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::ToolCall { id, name, .. }
                if id == "tc1" && name == "search"
        ));
    }

    #[tokio::test]
    async fn tool_result_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(&handle, ep.clone()).await.unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ToolActivityEvent::Result(ToolResultEvent {
                correlation_id: "c1".into(),
                tool_call_id: "tc1".into(),
                name: "search".into(),
                output: "found it".into(),
                is_error: false,
            }),
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::ToolResult { tool_call_id, name, output, is_error }
                if tool_call_id == "tc1" && name == "search" && output == "found it" && !is_error
        ));
    }

    #[tokio::test]
    async fn intermediate_maps_to_broadcast_response() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(&handle, ep.clone()).await.unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            IntermediateEvent {
                correlation_id: "c1".into(),
                content: "thinking...".into(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::BroadcastResponse { content }
                if content == "thinking..."
        ));
    }

    #[tokio::test]
    async fn notice_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(&handle, ep).await.unwrap();

        pub_.publish(
            topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
            NoticeEvent {
                message: "reloading".into(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Notice { message }
                if message == "reloading"
        ));
    }
}
