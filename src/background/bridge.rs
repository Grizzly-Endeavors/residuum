//! Bridge task: reads background results from the spawner's mpsc channel and
//! publishes them as `AgentResultEvent` on the bus.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::background::types::BackgroundResult;
use crate::bus::{
    AgentResultEvent, EventTrigger, HEARTBEAT_OK, HEARTBEAT_URGENT, Publisher, ResultDisposition,
    topics,
};

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
    crate::util::spawn_supervised(
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
    // Sentinel strings are the agreed protocol for an agent to state what should
    // happen with its own result. This is intentional protocol, not content-sniffing.
    // Agents MUST be able to exit silently when they have nothing to report, and
    // MUST be able to escalate when they find something that cannot wait.
    // The sentinels are distinctive so ordinary prose about an urgent-sounding
    // topic does not trip them.
    // This value cannot be set earlier in the pipeline by producers; it must be
    // determined here.
    let disposition =
        if matches!(result.source, EventTrigger::Pulse) && result.summary.contains(HEARTBEAT_OK) {
            ResultDisposition::Silent
        } else if result.summary.contains(HEARTBEAT_URGENT) {
            ResultDisposition::Urgent
        } else {
            ResultDisposition::Normal
        };

    AgentResultEvent {
        task_id: result.id.clone(),
        source_label: result.source_label.clone(),
        agent_preset: result.agent_preset.clone(),
        source: result.source.clone(),
        disposition,
        status: result.status.clone(),
        summary: result.summary.clone(),
        transcript_path: result.transcript_path.clone(),
        timestamp: result.timestamp.with_timezone(&tz).naive_local(),
    }
}

#[cfg(test)]
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
        assert_eq!(event.disposition, ResultDisposition::Normal);
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
        assert_eq!(event.disposition, ResultDisposition::Silent);
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
        assert!(matches!(event.status, AgentResultStatus::Completed));
        assert_eq!(event.summary, "done");
        assert_eq!(event.disposition, ResultDisposition::Normal);

        drop(tx);
        handle.await.unwrap();
    }

    #[test]
    fn convert_pulse_result_without_sentinel() {
        let result = BackgroundResult {
            id: "pulse-2".into(),
            source_label: "pulse:health".into(),
            source: EventTrigger::Pulse,
            summary: "checked 3 items".into(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.disposition, ResultDisposition::Normal);
    }

    fn sample_result(source: EventTrigger, summary: &str) -> BackgroundResult {
        BackgroundResult {
            id: "s-1".into(),
            source_label: "pulse:health".into(),
            source,
            summary: summary.into(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),
            agent_preset: PresetName::from("general-purpose"),
        }
    }

    #[test]
    fn urgent_sentinel_escalates_a_pulse_result() {
        let result = sample_result(EventTrigger::Pulse, "disk at 98%\nHEARTBEAT_URGENT");
        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.disposition, ResultDisposition::Urgent);
    }

    #[test]
    fn urgent_sentinel_escalates_an_action_result() {
        let result = sample_result(EventTrigger::Action, "deploy failed\nHEARTBEAT_URGENT");
        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.disposition, ResultDisposition::Urgent);
    }

    #[test]
    fn prose_about_urgency_does_not_escalate() {
        let result = sample_result(
            EventTrigger::Pulse,
            "An urgent-sounding email arrived. URGENT: reply to it. Nothing is actually on fire.",
        );
        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(
            event.disposition,
            ResultDisposition::Normal,
            "only the exact sentinel may escalate, not the word 'urgent'"
        );
    }

    #[test]
    fn silent_sentinel_wins_over_urgent_on_a_pulse() {
        let result = sample_result(EventTrigger::Pulse, "HEARTBEAT_OK HEARTBEAT_URGENT");
        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(
            event.disposition,
            ResultDisposition::Silent,
            "an agent that says it has nothing to report is taken at its word"
        );
    }

    #[test]
    fn convert_non_pulse_result_with_heartbeat_sentinel() {
        let result = BackgroundResult {
            id: "action-1".into(),
            source_label: "action:check".into(),
            source: EventTrigger::Action,
            summary: "HEARTBEAT_OK".into(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let event = convert_to_agent_result(&result, chrono_tz::UTC);
        assert_eq!(event.disposition, ResultDisposition::Normal);
    }
}
