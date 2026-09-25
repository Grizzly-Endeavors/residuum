//! Publishing session activity on the bus's [`topics::Sessions`] topic.
//!
//! Every place in the session machinery that changes a run's lifecycle state
//! or produces a turn event funnels through [`publish_session_event`], so the
//! web UI sees the same stream regardless of which code path produced it.

use crate::bus::{Publisher, SessionAddress, SessionEvent, SessionEventKind, topics};

/// Publish one event for `address`'s run `run_id`.
///
/// A publish only fails when the broker has shut down (gateway teardown), so
/// the failure is logged rather than propagated: losing a UI event must never
/// affect the run itself. Lifecycle and error events log at `warn`; the
/// high-frequency turn-stream events (tool activity, intermediate text) log
/// at `debug`, matching how the main agent treats its own turn events, so a
/// shutting-down broker doesn't produce a warning per tool call.
pub(crate) async fn publish_session_event(
    publisher: &Publisher,
    address: &SessionAddress,
    run_id: &str,
    kind: SessionEventKind,
) {
    let high_frequency = matches!(
        kind,
        SessionEventKind::ToolCall(_)
            | SessionEventKind::ToolResult(_)
            | SessionEventKind::Intermediate { .. }
    );
    let event = SessionEvent {
        address: address.clone(),
        run_id: run_id.to_string(),
        kind,
    };
    if let Err(e) = publisher.publish(topics::Sessions, event).await {
        if high_frequency {
            tracing::debug!(error = %e, address = %address, run_id, "failed to publish session turn event");
        } else {
            tracing::warn!(error = %e, address = %address, run_id, "failed to publish session event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_tagged_event_on_sessions_topic() {
        let handle = crate::bus::spawn_broker();
        let mut sub: crate::bus::Subscriber<SessionEvent> =
            handle.subscribe(topics::Sessions).await.unwrap();

        publish_session_event(
            &handle.publisher(),
            &SessionAddress::from("spawned-x-0001"),
            "run-1",
            SessionEventKind::Error {
                message: "boom".to_string(),
                details: None,
            },
        )
        .await;

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address.as_ref(), "spawned-x-0001");
        assert_eq!(event.run_id, "run-1");
        assert!(
            matches!(event.kind, SessionEventKind::Error { ref message, .. } if message == "boom"),
            "the payload should round-trip unchanged"
        );
    }
}
