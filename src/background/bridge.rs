//! Bridge task: reads background results from the spawner's mpsc channel and
//! publishes them as `AgentResultEvent` on the bus.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::background::types::BackgroundResult;
use crate::bus::{AgentResultEvent, EventTrigger, HeartbeatStatus, Publisher, topics};

/// Shared receiver for the bridge task, enabling supervision restarts.
pub(crate) type SharedResultReceiver = Arc<tokio::sync::Mutex<mpsc::Receiver<BackgroundResult>>>;

/// Spawn the bridge task that forwards background results to the bus.
///
/// Uses a shared receiver so the bridge can be restarted by a supervisor
/// without losing the channel. The receiver is held behind an `Arc<Mutex>`
/// and locked for the duration of each run.
pub(crate) fn spawn_result_bridge(
    result_rx: SharedResultReceiver,
    publisher: Publisher,
    tz: chrono_tz::Tz,
) -> tokio::task::JoinHandle<()> {
    crate::bus::supervision::spawn_supervised(
        "result-bridge",
        move || {
            let rx = Arc::clone(&result_rx);
            let pub_ = publisher.clone();
            async move {
                run_bridge(rx, pub_, tz).await;
            }
        },
        5,
        std::time::Duration::from_secs(1),
    )
}

/// Inner bridge loop: lock the shared receiver and forward results to the bus.
async fn run_bridge(result_rx: SharedResultReceiver, publisher: Publisher, tz: chrono_tz::Tz) {
    tracing::debug!("result bridge started");
    let mut rx = result_rx.lock().await;
    while let Some(result) = rx.recv().await {
        let event = convert_to_agent_result(&result, tz);
        if let Err(e) = publisher.publish(topics::Background, event).await {
            tracing::warn!(
                task_id = %result.id,
                error = %e,
                "bridge failed to publish background result to bus"
            );
        }
    }
    tracing::info!("result bridge shutting down (channel closed)");
}

/// Convert a `BackgroundResult` to an `AgentResultEvent` for the bus.
fn convert_to_agent_result(result: &BackgroundResult, tz: chrono_tz::Tz) -> AgentResultEvent {
    // "HEARTBEAT_OK" is the agreed sentinel string that pulse agents include in
    // their summary to signal "nothing needs surfacing". Its presence suppresses
    // notification routing. This is intentional protocol, not content-sniffing.
    let heartbeat_status = if matches!(result.source, EventTrigger::Pulse)
        && result.summary.contains("HEARTBEAT_OK")
    {
        HeartbeatStatus::Ok
    } else {
        HeartbeatStatus::Substantive
    };

    AgentResultEvent {
        task_id: result.id.clone(),
        source_label: result.source_label.clone(),
        agent_preset: result.agent_preset.clone(),
        source: result.source.clone(),
        heartbeat_status,
        status: result.status.clone(),
        summary: result.summary.clone(),
        transcript_path: result.transcript_path.clone(),
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
    use crate::bus::{AgentResultStatus, PresetName};

    #[test]
    fn convert_completed_result() {
        let result = BackgroundResult {
            id: "bg-1".into(),
            source_label: "action:email_check".into(),
            source: EventTrigger::Action,
            summary: "3 new emails".into(),
            transcript_path: Some(PathBuf::from("/tmp/bg-1.log")),
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.task_id, "bg-1");
        assert_eq!(event.source_label, "action:email_check");
        assert!(matches!(event.status, AgentResultStatus::Completed));
        assert_eq!(event.heartbeat_status, HeartbeatStatus::Substantive);
    }

    #[test]
    fn convert_heartbeat_ok_result() {
        let result = BackgroundResult {
            id: "pulse-1".into(),
            source_label: "pulse:health".into(),
            source: EventTrigger::Pulse,
            summary: "HEARTBEAT_OK".into(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

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
            status: AgentResultStatus::Failed {
                error: "timeout".into(),
            },
            timestamp: Utc::now(),

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
        let mut subscriber = bus_handle.subscribe(topics::Background).await.unwrap();

        let shared_rx = Arc::new(tokio::sync::Mutex::new(rx));
        let handle = spawn_result_bridge(shared_rx, publisher, chrono_tz::UTC);

        let result = BackgroundResult {
            id: "bg-test".into(),
            source_label: "agent:test-task".into(),
            source: EventTrigger::Agent,
            summary: "done".into(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        tx.send(result).await.unwrap();

        let event: AgentResultEvent =
            tokio::time::timeout(std::time::Duration::from_millis(200), subscriber.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();

        assert_eq!(event.task_id, "bg-test");
        assert_eq!(event.source_label, "agent:test-task");

        drop(tx);
        handle.await.unwrap();
    }
}
