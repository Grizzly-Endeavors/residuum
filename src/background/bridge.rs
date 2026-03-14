//! Bridge task: reads background results from the spawner's mpsc channel and
//! publishes them as `BusEvent::AgentResult` on the bus.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::background::types::{BackgroundResult, ResultRouting, TaskStatus};
use crate::bus::{
    AgentResultEvent, AgentResultStatus, BusEvent, EventTrigger, HeartbeatStatus, Publisher,
    TopicId,
};

/// Spawn the bridge task that forwards background results to the bus.
///
/// Reads from `result_rx` (the spawner's output channel), converts each
/// `BackgroundResult` to an `AgentResultEvent`, and publishes it to
/// `TopicId::BackgroundResult`.
pub(crate) fn spawn_result_bridge(
    mut result_rx: mpsc::Receiver<BackgroundResult>,
    publisher: Publisher,
    tz: chrono_tz::Tz,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(result) = result_rx.recv().await {
            let event = convert_to_agent_result(&result, tz);
            if let Err(e) = publisher
                .publish(TopicId::BackgroundResult, BusEvent::AgentResult(event))
                .await
            {
                tracing::warn!(
                    task_id = %result.id,
                    error = %e,
                    "bridge failed to publish background result to bus"
                );
            }
        }
        tracing::info!("result bridge shutting down");
    })
}

/// Convert a `BackgroundResult` to an `AgentResultEvent` for the bus.
fn convert_to_agent_result(result: &BackgroundResult, tz: chrono_tz::Tz) -> AgentResultEvent {
    let heartbeat_status = if matches!(result.source, EventTrigger::Pulse)
        && result.summary.contains("HEARTBEAT_OK")
    {
        HeartbeatStatus::Ok
    } else {
        HeartbeatStatus::Substantive
    };

    let status = match &result.status {
        TaskStatus::Completed => AgentResultStatus::Completed,
        TaskStatus::Cancelled => AgentResultStatus::Cancelled,
        TaskStatus::Failed { error } => AgentResultStatus::Failed {
            error: error.clone(),
        },
    };

    let ResultRouting::Direct(channels) = &result.routing;

    AgentResultEvent {
        task_id: result.id.clone(),
        source_label: result.source_label.clone(),
        agent_preset: result.agent_preset.clone(),
        source: result.source.clone(),
        heartbeat_status,
        status,
        summary: result.summary.clone(),
        transcript_path: result.transcript_path.clone(),
        routing: channels.clone(),
        timestamp: result.timestamp.with_timezone(&tz).naive_local(),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::panic, reason = "test assertions")]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test assertions use wildcard for non-matching variants"
)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;

    use super::*;
    use crate::background::types::ResultRouting;
    use crate::bus::PresetName;

    #[test]
    fn convert_completed_result() {
        let result = BackgroundResult {
            id: "bg-1".into(),
            source_label: "action:email_check".into(),
            source: EventTrigger::Action,
            summary: "3 new emails".into(),
            transcript_path: Some(PathBuf::from("/tmp/bg-1.log")),
            status: TaskStatus::Completed,
            timestamp: Utc::now(),
            routing: ResultRouting::Direct(vec!["inbox".into()]),
            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.task_id, "bg-1");
        assert_eq!(event.source_label, "action:email_check");
        assert!(matches!(event.status, AgentResultStatus::Completed));
        assert_eq!(event.heartbeat_status, HeartbeatStatus::Substantive);
        assert_eq!(event.routing, vec!["inbox".to_string()]);
    }

    #[test]
    fn convert_heartbeat_ok_result() {
        let result = BackgroundResult {
            id: "pulse-1".into(),
            source_label: "pulse:health".into(),
            source: EventTrigger::Pulse,
            summary: "HEARTBEAT_OK".into(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: Utc::now(),
            routing: ResultRouting::Direct(vec!["inbox".into()]),
            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.heartbeat_status, HeartbeatStatus::Ok);
    }

    #[test]
    fn convert_failed_result() {
        let result = BackgroundResult {
            id: "bg-2".into(),
            source_label: "agent:deploy".into(),
            source: EventTrigger::Agent,
            summary: String::new(),
            transcript_path: None,
            status: TaskStatus::Failed {
                error: "timeout".into(),
            },
            timestamp: Utc::now(),
            routing: ResultRouting::Direct(vec![]),
            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        match event.status {
            AgentResultStatus::Failed { error } => assert_eq!(error, "timeout"),
            _ => panic!("expected Failed status"),
        }
    }

    #[tokio::test]
    async fn bridge_publishes_to_background_result_topic() {
        let (tx, rx) = mpsc::channel(8);
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(TopicId::BackgroundResult)
            .await
            .unwrap();

        let handle = spawn_result_bridge(rx, publisher, chrono_tz::UTC);

        let result = BackgroundResult {
            id: "bg-test".into(),
            source_label: "agent:test-task".into(),
            source: EventTrigger::Agent,
            summary: "done".into(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: Utc::now(),
            routing: ResultRouting::Direct(vec!["inbox".into()]),
            agent_preset: PresetName::from("general-purpose"),
        };

        tx.send(result).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_millis(200), subscriber.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            BusEvent::AgentResult(ar) => {
                assert_eq!(ar.task_id, "bg-test");
                assert_eq!(ar.source_label, "agent:test-task");
                assert_eq!(ar.routing, vec!["inbox".to_string()]);
            }
            _ => panic!("expected AgentResult event"),
        }

        drop(tx);
        handle.await.unwrap();
    }
}
